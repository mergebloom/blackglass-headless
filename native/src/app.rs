use crate::crypto::VaultCrypto;
use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};
use url::Url;

#[derive(Parser)]
#[command(name = "bgh", version, about = "Native Blackglass Headless client")]
struct Cli {
    #[arg(long, global = true)]
    profile: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Configure {
        #[arg(long)]
        server: String,
    },
    Login {
        #[arg(long)]
        email: String,
        #[arg(long)]
        password_stdin: bool,
    },
    Vault {
        #[command(subcommand)]
        command: VaultCommand,
    },
    Sync {
        #[command(subcommand)]
        command: SyncCommand,
    },
    Note {
        #[command(subcommand)]
        command: NoteCommand,
    },
    Mcp {
        #[arg(long)]
        auto_sync_seconds: Option<u64>,
    },
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },
    BuildInfo,
}
#[derive(Subcommand)]
enum ServiceCommand {
    Run {
        #[arg(long, default_value_t = 30)]
        interval_seconds: u64,
    },
}
#[derive(Subcommand)]
enum VaultCommand {
    List,
    Create {
        #[arg(long)]
        name: String,
        #[arg(long)]
        password_stdin: bool,
    },
    Connect {
        #[arg(long)]
        id: String,
        #[arg(long)]
        path: PathBuf,
        #[arg(long)]
        data_host: Option<String>,
        #[arg(long)]
        password_stdin: bool,
    },
}
#[derive(Subcommand)]
enum SyncCommand {
    Once,
    Status,
    Watch {
        #[arg(long, default_value_t = 30)]
        interval_seconds: u64,
    },
}
#[derive(Subcommand)]
enum NoteCommand {
    List,
    Search {
        query: String,
    },
    Read {
        path: String,
    },
    Write {
        path: String,
        #[arg(long)]
        expected_sha256: Option<String>,
    },
}

#[derive(Default, Serialize, Deserialize)]
struct Profile {
    server: String,
    token: Option<String>,
    vault: Option<VaultConfig>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct VaultConfig {
    pub id: String,
    pub host: String,
    pub salt: String,
    pub base_key: String,
    pub root: PathBuf,
    pub applied_version: i64,
    #[serde(default)]
    pub known: BTreeMap<String, String>,
}

fn profile_path(cli: &Cli) -> Result<PathBuf> {
    if let Some(path) = &cli.profile {
        return Ok(path.clone());
    }
    let home = std::env::var_os("HOME").context("HOME is unset; pass --profile")?;
    Ok(PathBuf::from(home).join(".config/blackglass-headless-native/profile.json"))
}
fn prepare_profile_dir(path: &Path) -> Result<()> {
    let parent = path.parent().context("profile path has no parent")?;
    if !parent.exists() {
        fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = fs::symlink_metadata(parent)?;
        if meta.file_type().is_symlink() || !meta.is_dir() || meta.permissions().mode() & 0o077 != 0
        {
            bail!("profile directory must be a real owner-only directory (mode 0700)");
        }
    }
    Ok(())
}
fn lock_profile(path: &Path) -> Result<fs::File> {
    prepare_profile_dir(path)?;
    let lock_path = path.with_extension("lock");
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(lock_path)?;
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        // Kernel-owned advisory lock releases automatically after process death.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            bail!("profile is already in use");
        }
    }
    Ok(file)
}
fn read_profile(path: &Path) -> Result<Profile> {
    if !path.exists() {
        return Ok(Profile::default());
    }
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        bail!("profile must not be a symlink");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::metadata(path)?.permissions().mode() & 0o077 != 0 {
            bail!("profile must be mode 0600");
        }
    }
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}
fn write_profile(path: &Path, profile: &Profile) -> Result<()> {
    let parent = path.parent().context("profile path has no parent")?;
    prepare_profile_dir(path)?;
    let tmp = path.with_extension("json.tmp");
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&tmp)
        .context("stale .tmp profile; inspect it before retrying")?;
    file.write_all(&serde_json::to_vec_pretty(profile)?)?;
    file.sync_all()?;
    fs::rename(&tmp, path)?;
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}
fn password(from_stdin: bool, prompt: &str) -> Result<String> {
    let mut secret = if from_stdin {
        let mut text = String::new();
        io::stdin().read_to_string(&mut text)?;
        text
    } else {
        rpassword::prompt_password(prompt)?
    };
    if from_stdin {
        secret = secret.trim_end_matches(['\r', '\n']).to_owned();
    }
    if secret.is_empty() {
        bail!("empty password rejected");
    }
    Ok(secret)
}
pub fn control_origin(server: &str) -> Result<Url> {
    let url = Url::parse(server)?;
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
    if url.scheme() != "https" && !(local && url.scheme() == "http") {
        bail!("server must use HTTPS (HTTP only on loopback)");
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        bail!("server must be an origin without credentials or path");
    }
    Ok(url)
}
async fn post(server: &str, route: &str, body: Value) -> Result<Value> {
    let url = control_origin(server)?.join(route.trim_start_matches('/'))?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let mut response = client
        .post(url)
        .header("Origin", "app://obsidian.md")
        .json(&body)
        .send()
        .await?;
    if !response.status().is_success() {
        bail!("{} failed ({})", route, response.status());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len().saturating_add(chunk.len()) > 2 * 1024 * 1024 {
            bail!("{} response exceeds 2 MiB bound", route);
        }
        body.extend_from_slice(&chunk);
    }
    let value: Value = serde_json::from_slice(&body)?;
    if let Some(error) = value.get("error") {
        bail!("{}: {}", route, error);
    }
    Ok(value)
}
fn token(profile: &Profile) -> Result<&str> {
    profile.token.as_deref().context("not signed in")
}
fn vaults(body: &Value) -> Vec<&Value> {
    let mut all = Vec::new();
    if let Some(v) = body["vaults"].as_array() {
        all.extend(v);
    }
    if let Some(v) = body["shared"].as_array() {
        all.extend(v);
    }
    all
}

pub async fn run() -> Result<()> {
    let cli = Cli::parse();
    if matches!(cli.command, Command::BuildInfo) {
        println!(
            "{}",
            json!({"version":env!("CARGO_PKG_VERSION"),
                "target_os":std::env::consts::OS,"target_arch":std::env::consts::ARCH,
                "source_sha256":option_env!("BLACKGLASS_SOURCE_SHA256").unwrap_or("unbound-development-build")})
        );
        return Ok(());
    }
    let path = profile_path(&cli)?;
    #[cfg(unix)]
    if !matches!(&cli.command, Command::Service { .. }) && service_available(&path).await {
        if let Command::Mcp { auto_sync_seconds } = &cli.command {
            return mcp_proxy(&path, *auto_sync_seconds).await;
        }
        if let Some(request) = command_service_request(&cli.command)? {
            let result = service_call(&path, request).await?;
            match &cli.command {
                Command::Sync {
                    command: SyncCommand::Once,
                } => println!(
                    "synchronized through revision {}",
                    result["applied_revision"]
                ),
                Command::Sync {
                    command: SyncCommand::Status,
                } => println!(
                    "{}: applied revision {}",
                    result["root"].as_str().unwrap_or(""),
                    result["applied_revision"]
                ),
                _ => println!("{}", result),
            }
            return Ok(());
        }
    }
    let _lock = lock_profile(&path)?;
    let mut profile = read_profile(&path)?;
    let needs_vault_lock = matches!(
        &cli.command,
        Command::Sync { .. } | Command::Note { .. } | Command::Mcp { .. } | Command::Service { .. }
    );
    let _vault_lock = if needs_vault_lock {
        let vault = profile.vault.as_ref().context("no vault connected")?;
        Some(crate::root::lock(&vault.root, &vault.id, &path)?)
    } else {
        None
    };
    match cli.command {
        Command::Configure { server } => {
            let origin = control_origin(&server)?.to_string();
            if !profile.server.is_empty() && profile.server != origin {
                profile.token = None;
                profile.vault = None;
            }
            profile.server = origin;
            write_profile(&path, &profile)?;
            println!("configured {}", profile.server);
        }
        Command::Login {
            email,
            password_stdin,
        } => {
            let secret = password(password_stdin, "Account password: ")?;
            let body = post(
                &profile.server,
                "/user/signin",
                json!({"email":email,"password":secret}),
            )
            .await?;
            profile.token = Some(
                body["token"]
                    .as_str()
                    .context("signin response missing token")?
                    .to_owned(),
            );
            write_profile(&path, &profile)?;
            println!(
                "signed in as {}",
                body["email"].as_str().unwrap_or("account")
            );
        }
        Command::Vault { command } => match command {
            VaultCommand::List => {
                let body = post(
                    &profile.server,
                    "/vault/list",
                    json!({"token":token(&profile)?,"supported_encryption_version":3}),
                )
                .await?;
                for item in vaults(&body) {
                    println!(
                        "{}\t{}\t{}",
                        item["id"].as_str().unwrap_or("?"),
                        item["name"].as_str().unwrap_or("?"),
                        if item["keyhash"].is_null() {
                            "managed: unsupported"
                        } else {
                            "custom E2EE"
                        }
                    );
                }
            }
            VaultCommand::Create {
                name,
                password_stdin,
            } => {
                let secret = password(password_stdin, "Vault password: ")?;
                let mut random = [0u8; 16];
                rand::thread_rng().fill_bytes(&mut random);
                use base64::Engine;
                let salt = base64::engine::general_purpose::STANDARD.encode(random);
                let crypto = VaultCrypto::derive(&secret, &salt)?;
                let body = post(&profile.server, "/vault/create", json!({"token":token(&profile)?,"name":name,
                    "keyhash":crypto.key_hash(),"salt":salt,"region":"Blackglass Server","encryption_version":3})).await?;
                println!(
                    "created {} ({})",
                    body["name"].as_str().unwrap_or("vault"),
                    body["id"].as_str().unwrap_or("?")
                );
            }
            VaultCommand::Connect {
                id,
                path: root,
                data_host,
                password_stdin,
            } => {
                let body = post(
                    &profile.server,
                    "/vault/list",
                    json!({"token":token(&profile)?,"supported_encryption_version":3}),
                )
                .await?;
                let item = vaults(&body)
                    .into_iter()
                    .find(|v| v["id"] == id)
                    .context("vault not found")?;
                if item["encryption_version"] != 3 || item["keyhash"].is_null() {
                    bail!("only custom E2EE v3 vaults are supported");
                }
                let salt = item["salt"].as_str().context("vault missing salt")?;
                let secret = password(password_stdin, "Vault password: ")?;
                let crypto = VaultCrypto::derive(&secret, salt)?;
                if item["keyhash"].as_str() != Some(crypto.key_hash()) {
                    bail!("wrong vault password");
                }
                let host = item["host"].as_str().context("vault missing data host")?;
                crate::sync::validate_data_host(&profile.server, host, data_host.as_deref())?;
                post(
                    &profile.server,
                    "/vault/access",
                    json!({"token":token(&profile)?,"vault_uid":id,
                    "keyhash":crypto.key_hash(),"host":host,"encryption_version":3}),
                )
                .await?;
                fs::create_dir_all(&root)?;
                let root = fs::canonicalize(root)?;
                if root.read_dir()?.next().is_some() {
                    bail!("connect requires an empty local directory");
                }
                let _new_vault_lock = crate::root::lock(&root, &id, &path)?;
                let mut base = [0u8; 32];
                use scrypt::{Params, scrypt};
                use unicode_normalization::UnicodeNormalization;
                scrypt(
                    secret.nfkc().collect::<String>().as_bytes(),
                    salt.nfkc().collect::<String>().as_bytes(),
                    &Params::new(15, 8, 1, 32)?,
                    &mut base,
                )
                .map_err(|_| anyhow::anyhow!("key derivation failed"))?;
                profile.vault = Some(VaultConfig {
                    id,
                    host: host.to_owned(),
                    salt: salt.to_owned(),
                    base_key: hex::encode(base),
                    root,
                    applied_version: 0,
                    known: BTreeMap::new(),
                });
                write_profile(&path, &profile)?;
                println!("connected; run bgh sync once");
            }
        },
        Command::Sync { command } => match command {
            SyncCommand::Once => {
                let server = profile.server.clone();
                let token = token(&profile)?.to_owned();
                let vault = profile.vault.as_mut().context("no vault connected")?;
                let mut journal = crate::state::Journal::open(&path, vault)?;
                let result =
                    crate::sync::once_with_checkpoint(&server, &token, vault, &mut |state| {
                        journal.checkpoint(state)
                    })
                    .await;
                let revision = result?;
                println!("synchronized through revision {revision}");
            }
            SyncCommand::Status => {
                let vault = profile.vault.as_mut().context("no vault connected")?;
                let _journal = crate::state::Journal::open(&path, vault)?;
                println!(
                    "{}: applied revision {}",
                    vault.root.display(),
                    vault.applied_version
                );
            }
            SyncCommand::Watch { interval_seconds } => {
                if !(1..=3600).contains(&interval_seconds) {
                    bail!("interval must be 1-3600 seconds");
                }
                loop {
                    let server = profile.server.clone();
                    let token = token(&profile)?.to_owned();
                    let vault = profile.vault.as_mut().context("no vault connected")?;
                    let mut journal = crate::state::Journal::open(&path, vault)?;
                    let result =
                        crate::sync::once_with_checkpoint(&server, &token, vault, &mut |state| {
                            journal.checkpoint(state)
                        })
                        .await;
                    println!("synchronized through revision {}", result?);
                    tokio::select! {
                        _ = tokio::signal::ctrl_c() => break,
                        _ = tokio::time::sleep(std::time::Duration::from_secs(interval_seconds)) => {}
                    }
                }
            }
        },
        Command::Note { command } => {
            let root = &profile.vault.as_ref().context("no vault connected")?.root;
            let result = match command {
                NoteCommand::List => crate::notes::list(root)?,
                NoteCommand::Search { query } => crate::notes::search(root, &query)?,
                NoteCommand::Read { path } => crate::notes::read(root, &path)?,
                NoteCommand::Write {
                    path,
                    expected_sha256,
                } => {
                    let mut content = String::new();
                    io::stdin()
                        .take(1024 * 1024 + 1)
                        .read_to_string(&mut content)?;
                    crate::notes::write(root, &path, &content, expected_sha256.as_deref())?
                }
            };
            println!("{}", serde_json::to_string(&result)?);
        }
        Command::Mcp { auto_sync_seconds } => {
            let vault = profile.vault.as_mut().context("no vault connected")?;
            drop(crate::state::Journal::open(&path, vault)?);
            mcp(profile, &path, auto_sync_seconds).await?
        }
        Command::Service {
            command: ServiceCommand::Run { interval_seconds },
        } => service(profile, &path, interval_seconds).await?,
        Command::BuildInfo => unreachable!(),
    }
    Ok(())
}

#[cfg(unix)]
fn service_socket(path: &Path) -> PathBuf {
    path.with_extension("sock")
}

#[cfg(unix)]
async fn service_available(path: &Path) -> bool {
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        tokio::net::UnixStream::connect(service_socket(path)),
    )
    .await
    .is_ok_and(|result| result.is_ok())
}

#[cfg(unix)]
fn command_service_request(command: &Command) -> Result<Option<Value>> {
    Ok(match command {
        Command::Sync {
            command: SyncCommand::Once,
        } => Some(json!({"op":"sync_once"})),
        Command::Sync {
            command: SyncCommand::Status,
        } => Some(json!({"op":"sync_status"})),
        Command::Note { command } => Some(match command {
            NoteCommand::List => json!({"op":"note_list"}),
            NoteCommand::Search { query } => json!({"op":"note_search","query":query}),
            NoteCommand::Read { path } => json!({"op":"note_read","path":path}),
            NoteCommand::Write {
                path,
                expected_sha256,
            } => {
                let mut content = String::new();
                io::stdin()
                    .take(1024 * 1024 + 1)
                    .read_to_string(&mut content)?;
                if content.len() > 1024 * 1024 {
                    bail!("note exceeds 1 MiB limit");
                }
                json!({"op":"note_write","path":path,"content":content,"expected_sha256":expected_sha256})
            }
        }),
        _ => None,
    })
}

#[cfg(unix)]
async fn service_call(path: &Path, request: Value) -> Result<Value> {
    use tokio::io::AsyncWriteExt;
    let mut stream = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        tokio::net::UnixStream::connect(service_socket(path)),
    )
    .await
    .context("local service connection timed out")??;
    let body = serde_json::to_vec(&request)?;
    if body.len() > 2 * 1024 * 1024 {
        bail!("local service request exceeds 2 MiB");
    }
    stream.write_all(&body).await?;
    stream.write_all(b"\n").await?;
    let mut reader = tokio::io::BufReader::new(stream);
    let mut response = Vec::new();
    let mut oversized = false;
    let frame = tokio::time::timeout(
        std::time::Duration::from_secs(16 * 60),
        read_mcp_frame(&mut reader, &mut response, &mut oversized),
    )
    .await
    .context("local service response timed out")??
    .context("local service returned an empty response")?;
    if frame.1 {
        bail!("local service returned an empty or oversized response");
    }
    let result: Value = serde_json::from_slice(&frame.0)?;
    if result["ok"] != true {
        bail!(
            "{}",
            result["error"].as_str().unwrap_or("local service failed")
        );
    }
    Ok(result["value"].clone())
}

#[cfg(unix)]
struct SocketLease {
    path: PathBuf,
    device: u64,
    inode: u64,
}

#[cfg(unix)]
impl Drop for SocketLease {
    fn drop(&mut self) {
        use std::os::unix::fs::MetadataExt;
        if let Ok(metadata) = fs::symlink_metadata(&self.path)
            && (metadata.dev(), metadata.ino()) == (self.device, self.inode)
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(unix)]
fn check_service_peer(stream: &tokio::net::UnixStream) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        let mut credentials = libc::ucred {
            pid: 0,
            uid: 0,
            gid: 0,
        };
        let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        let result = unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&mut credentials as *mut libc::ucred).cast(),
                &mut length,
            )
        };
        if result != 0
            || length as usize != std::mem::size_of::<libc::ucred>()
            || credentials.uid != unsafe { libc::geteuid() }
        {
            bail!("local service peer is not the profile owner");
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = stream;
    Ok(())
}

#[cfg(unix)]
async fn send_service_result(
    stream: &mut tokio::net::UnixStream,
    result: Result<Value>,
) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    let response = match result {
        Ok(value) => json!({"ok":true,"value":value}),
        Err(error) => json!({"ok":false,"error":error.to_string()}),
    };
    let bytes = serde_json::to_vec(&response)?;
    stream.write_all(&bytes).await?;
    stream.write_all(b"\n").await?;
    stream.flush().await?;
    Ok(())
}

#[cfg(unix)]
async fn read_service_request(
    stream: tokio::net::UnixStream,
) -> Result<(tokio::net::UnixStream, Value)> {
    let mut reader = tokio::io::BufReader::new(stream);
    let mut line = Vec::new();
    let mut oversized = false;
    let frame = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        read_mcp_frame(&mut reader, &mut line, &mut oversized),
    )
    .await
    .context("local service request timed out")??
    .context("local service request was empty")?;
    if frame.1 {
        bail!("local service request exceeds 2 MiB");
    }
    Ok((reader.into_inner(), serde_json::from_slice(&frame.0)?))
}

#[cfg(unix)]
fn service_note(profile: &Profile, request: &Value, syncing: bool) -> Result<Value> {
    let vault = profile.vault.as_ref().context("no vault connected")?;
    let root = &vault.root;
    match request["op"].as_str().unwrap_or("") {
        "sync_status" => {
            Ok(json!({"root":root,"applied_revision":vault.applied_version,"in_progress":syncing}))
        }
        "note_list" if !syncing => crate::notes::list(root),
        "note_search" if !syncing => {
            crate::notes::search(root, request["query"].as_str().context("query required")?)
        }
        "note_read" if !syncing => {
            crate::notes::read(root, request["path"].as_str().context("path required")?)
        }
        "note_write" if !syncing => crate::notes::write(
            root,
            request["path"].as_str().context("path required")?,
            request["content"].as_str().context("content required")?,
            request["expected_sha256"].as_str(),
        ),
        "note_list" | "note_search" | "note_read" | "note_write" => {
            bail!("Sync is running; retry note operation")
        }
        _ => bail!("unsupported local service operation"),
    }
}

#[cfg(unix)]
async fn service(mut profile: Profile, path: &Path, interval_seconds: u64) -> Result<()> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
    if !(1..=3600).contains(&interval_seconds) {
        bail!("interval must be 1-3600 seconds");
    }
    let vault = profile.vault.as_mut().context("no vault connected")?;
    drop(crate::state::Journal::open(path, vault)?);
    let socket = service_socket(path);
    match fs::symlink_metadata(&socket) {
        Ok(metadata) => {
            if !metadata.file_type().is_socket() || metadata.uid() != unsafe { libc::geteuid() } {
                bail!("local service socket path is occupied by an unexpected file");
            }
            if tokio::net::UnixStream::connect(&socket).await.is_ok() {
                bail!("local service is already running");
            }
            // The caller holds the exclusive profile lock, so a same-owner
            // socket that cannot accept connections is a crashed service lease.
            fs::remove_file(&socket)?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let listener = tokio::net::UnixListener::bind(&socket)?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;
    let metadata = fs::symlink_metadata(&socket)?;
    let _lease = SocketLease {
        path: socket,
        device: metadata.dev(),
        inode: metadata.ino(),
    };
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(interval_seconds));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut active_sync: Option<McpSync> = None;
    let mut waiting: Option<tokio::net::UnixStream> = None;
    let mut failures = 0u32;
    let mut next_retry = tokio::time::Instant::now();
    loop {
        enum Event {
            Accept(std::io::Result<(tokio::net::UnixStream, tokio::net::unix::SocketAddr)>),
            Tick,
            SyncDone(std::result::Result<(VaultConfig, Result<i64>), tokio::task::JoinError>),
            Stop,
        }
        let event = tokio::select! {
            accepted = listener.accept() => Event::Accept(accepted),
            _ = ticker.tick() => Event::Tick,
            done = async { (&mut active_sync.as_mut().expect("guarded service Sync").worker).await }, if active_sync.is_some() => Event::SyncDone(done),
            _ = tokio::signal::ctrl_c() => Event::Stop,
            _ = terminate.recv() => Event::Stop,
        };
        match event {
            Event::Stop => break,
            Event::Tick => {
                if active_sync.is_none() && tokio::time::Instant::now() >= next_retry {
                    match start_mcp_sync(&profile, path) {
                        Ok(worker) => active_sync = Some(worker),
                        Err(error) => {
                            failures = failures.saturating_add(1);
                            let seconds = interval_seconds
                                .saturating_mul(1u64 << failures.min(5))
                                .min(900);
                            next_retry = tokio::time::Instant::now()
                                + std::time::Duration::from_secs(seconds);
                            eprintln!("background Sync could not start: {error:#}");
                        }
                    }
                }
            }
            Event::SyncDone(done) => {
                active_sync = None;
                let result = finish_mcp_sync(
                    &mut profile,
                    path,
                    done.context("service Sync worker failed")?,
                )?;
                if result.is_ok() {
                    failures = 0;
                    next_retry = tokio::time::Instant::now();
                } else {
                    failures = failures.saturating_add(1);
                    let seconds = interval_seconds
                        .saturating_mul(1u64 << failures.min(5))
                        .min(900);
                    next_retry =
                        tokio::time::Instant::now() + std::time::Duration::from_secs(seconds);
                }
                if let Some(mut stream) = waiting.take() {
                    send_service_result(
                        &mut stream,
                        result.map(|revision| json!({"applied_revision":revision})),
                    )
                    .await?;
                } else if let Err(error) = result {
                    eprintln!("background Sync failed: {error:#}");
                }
            }
            Event::Accept(accepted) => {
                let (stream, _) = accepted?;
                if let Err(error) = check_service_peer(&stream) {
                    let mut stream = stream;
                    let _ = send_service_result(&mut stream, Err(error)).await;
                    continue;
                }
                let (mut stream, request) = match read_service_request(stream).await {
                    Ok(value) => value,
                    Err(_) => continue,
                };
                match request["op"].as_str().unwrap_or("") {
                    "sync_once" => {
                        if active_sync.is_some() {
                            send_service_result(
                                &mut stream,
                                Err(anyhow::anyhow!("Sync is already running")),
                            )
                            .await?;
                        } else {
                            match start_mcp_sync(&profile, path) {
                                Ok(worker) => {
                                    active_sync = Some(worker);
                                    waiting = Some(stream);
                                }
                                Err(error) => send_service_result(&mut stream, Err(error)).await?,
                            }
                        }
                    }
                    "sync_cancel" => {
                        let result = if let Some(worker) = &active_sync {
                            worker.cancel.send(true)?;
                            Ok(
                                json!({"cancelled":true,"outcome":"indeterminate until Sync settles"}),
                            )
                        } else {
                            Err(anyhow::anyhow!("no Sync is running"))
                        };
                        send_service_result(&mut stream, result).await?;
                    }
                    _ => {
                        let result = service_note(&profile, &request, active_sync.is_some());
                        send_service_result(&mut stream, result).await?;
                    }
                }
            }
        }
    }
    if let Some(worker) = active_sync {
        let _ = worker.cancel.send(true);
        let _ = finish_mcp_sync(
            &mut profile,
            path,
            worker
                .worker
                .await
                .context("service Sync worker failed at shutdown")?,
        )?;
    }
    Ok(())
}

#[cfg(not(unix))]
async fn service(_profile: Profile, _path: &Path, _interval_seconds: u64) -> Result<()> {
    bail!("local service requires Unix sockets")
}

#[cfg(unix)]
fn mcp_service_request(name: &str, args: &Value) -> Result<Value> {
    Ok(match name {
        "note_list" => json!({"op":"note_list"}),
        "note_read" => json!({"op":"note_read","path":args["path"]}),
        "note_search" => json!({"op":"note_search","query":args["query"]}),
        "note_write" => {
            json!({"op":"note_write","path":args["path"],"content":args["content"],"expected_sha256":args["expected_sha256"]})
        }
        "sync_run" => json!({"op":"sync_once"}),
        "sync_status" => json!({"op":"sync_status"}),
        "sync_cancel" => json!({"op":"sync_cancel"}),
        _ => bail!("unknown tool"),
    })
}

#[cfg(unix)]
async fn mcp_proxy(path: &Path, auto_sync_seconds: Option<u64>) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    if auto_sync_seconds.is_some_and(|seconds| !(1..=3600).contains(&seconds)) {
        bail!("auto-sync interval must be 1-3600 seconds");
    }
    // The independent service owns polling; this process only adapts stdio.
    let mut reader = tokio::io::BufReader::new(tokio::io::stdin());
    let mut frame = Vec::new();
    let mut oversized = false;
    let mut stdout = tokio::io::stdout();
    let mut pending: Option<(Value, tokio::task::JoinHandle<Result<Value>>)> = None;
    loop {
        enum Event {
            Line(Option<(Vec<u8>, bool)>),
            Done(std::result::Result<Result<Value>, tokio::task::JoinError>),
        }
        let event = tokio::select! {
            line = read_mcp_frame(&mut reader, &mut frame, &mut oversized) => Event::Line(line?),
            done = async { (&mut pending.as_mut().expect("guarded MCP service call").1).await }, if pending.is_some() => Event::Done(done),
        };
        let (bytes, too_large) = match event {
            Event::Done(done) => {
                let (id, _) = pending.take().context("missing MCP service call")?;
                let result = done.context("MCP service worker failed")?;
                let response = mcp_tool_result(id, result);
                stdout.write_all(response.to_string().as_bytes()).await?;
                stdout.write_all(b"\n").await?;
                stdout.flush().await?;
                continue;
            }
            Event::Line(Some(frame)) => frame,
            Event::Line(None) => break,
        };
        if too_large {
            stdout.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":null,\"error\":{\"code\":-32600,\"message\":\"Request exceeds 2 MiB\"}}\n").await?;
            stdout.flush().await?;
            continue;
        }
        let request: Value = match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(_) => {
                stdout.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":null,\"error\":{\"code\":-32700,\"message\":\"Parse error\"}}\n").await?;
                stdout.flush().await?;
                continue;
            }
        };
        let id = request.get("id").cloned();
        if !request.is_object()
            || request["jsonrpc"] != "2.0"
            || !request["method"].is_string()
            || id
                .as_ref()
                .is_some_and(|id| !(id.is_string() || id.is_i64() || id.is_u64() || id.is_null()))
        {
            let response = json!({"jsonrpc":"2.0","id":id.filter(|id| id.is_string() || id.is_i64() || id.is_u64()).unwrap_or(Value::Null),"error":{"code":-32600,"message":"Invalid Request"}});
            stdout.write_all(response.to_string().as_bytes()).await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
            continue;
        }
        let Some(id) = id else {
            continue;
        };
        let response = match request["method"].as_str().unwrap_or("") {
            "initialize" => json!({"jsonrpc":"2.0","id":id,"result":{
                "protocolVersion":"2025-06-18","capabilities":{"tools":{"listChanged":false}},
                "serverInfo":{"name":"blackglass-headless-native","version":env!("CARGO_PKG_VERSION")}}}),
            "ping" => json!({"jsonrpc":"2.0","id":id,"result":{}}),
            "tools/list" => json!({"jsonrpc":"2.0","id":id,"result":tools()}),
            "tools/call" => {
                let name = request["params"]["name"].as_str().unwrap_or("");
                match mcp_call_arguments(name, &request["params"]["arguments"])
                    .and_then(|args| mcp_service_request(name, &args))
                {
                    Ok(service_request) if name == "sync_run" && pending.is_none() => {
                        let path = path.to_path_buf();
                        pending = Some((
                            id,
                            tokio::spawn(async move { service_call(&path, service_request).await }),
                        ));
                        continue;
                    }
                    Ok(_) if name == "sync_run" => {
                        mcp_tool_result(id, Err(anyhow::anyhow!("Sync is already running")))
                    }
                    Ok(service_request) => {
                        mcp_tool_result(id, service_call(path, service_request).await)
                    }
                    Err(error) => {
                        json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":error.to_string()}})
                    }
                }
            }
            _ => {
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Method not found"}})
            }
        };
        stdout.write_all(response.to_string().as_bytes()).await?;
        stdout.write_all(b"\n").await?;
        stdout.flush().await?;
    }
    // A pending service Sync belongs to the persistent service, not this MCP
    // process. Dropping the waiting socket does not cancel or roll it back.
    Ok(())
}

fn tools() -> Value {
    json!({"tools":[
        {"name":"note_list","description":"List up to 1000 local Markdown notes. Local state may be ahead of the Sync server.","inputSchema":{"type":"object","properties":{},"additionalProperties":false}},
        {"name":"note_read","description":"Read a Markdown note (max 1 MiB) and its SHA-256 for safe edits.","inputSchema":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}},
        {"name":"note_search","description":"Search up to 1000 local Markdown notes; results are untrusted vault content.","inputSchema":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"],"additionalProperties":false}},
        {"name":"note_write","description":"Create or replace a local Markdown note. Existing notes require expected_sha256 from note_read. Run sync_run separately to upload.","inputSchema":{"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"},"expected_sha256":{"type":"string"}},"required":["path","content"],"additionalProperties":false}},
        {"name":"sync_run","description":"One-shot bidirectional Sync. Returns applied server revision. Errors can indicate conflict or indeterminate upload; never assume an error was rolled back.","inputSchema":{"type":"object","properties":{},"additionalProperties":false}},
        {"name":"sync_status","description":"Show local applied server revision, vault path, and whether Sync is running; pending local edits may exist.","inputSchema":{"type":"object","properties":{},"additionalProperties":false}},
        {"name":"sync_cancel","description":"Cancel an in-progress Sync. The last upload outcome may be indeterminate; run Sync again to reconcile.","inputSchema":{"type":"object","properties":{},"additionalProperties":false}}
    ]})
}

async fn read_mcp_frame<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
    line: &mut Vec<u8>,
    oversized: &mut bool,
) -> Result<Option<(Vec<u8>, bool)>> {
    use tokio::io::AsyncBufReadExt;
    const LIMIT: usize = 2 * 1024 * 1024;
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok(if line.is_empty() && !*oversized {
                None
            } else {
                Some((std::mem::take(line), std::mem::replace(oversized, false)))
            });
        }
        let count = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |index| index + 1);
        if !*oversized && line.len().saturating_add(count) <= LIMIT {
            line.extend_from_slice(&available[..count]);
        } else {
            *oversized = true;
        }
        let complete = available[count - 1] == b'\n';
        reader.consume(count);
        if complete {
            return Ok(Some((
                std::mem::take(line),
                std::mem::replace(oversized, false),
            )));
        }
    }
}

fn mcp_call_arguments(name: &str, value: &Value) -> Result<Value> {
    let args = if value.is_null() {
        json!({})
    } else {
        value.clone()
    };
    let fields = args
        .as_object()
        .context("tool arguments must be an object")?;
    let allowed: &[&str] = match name {
        "note_list" | "sync_run" | "sync_status" | "sync_cancel" => &[],
        "note_read" => &["path"],
        "note_search" => &["query"],
        "note_write" => &["path", "content", "expected_sha256"],
        _ => bail!("unknown tool"),
    };
    if fields.keys().any(|key| !allowed.contains(&key.as_str())) {
        bail!("unknown tool argument");
    }
    for required in match name {
        "note_read" | "note_write" => &["path"][..],
        "note_search" => &["query"][..],
        _ => &[][..],
    } {
        if !fields.get(*required).is_some_and(Value::is_string) {
            bail!("required tool argument {required} must be a string");
        }
    }
    if name == "note_write" && !fields.get("content").is_some_and(Value::is_string) {
        bail!("required tool argument content must be a string");
    }
    if fields
        .get("expected_sha256")
        .is_some_and(|value| !value.is_string())
    {
        bail!("expected_sha256 must be a string");
    }
    Ok(args)
}

struct McpSync {
    worker: tokio::task::JoinHandle<(VaultConfig, Result<i64>)>,
    cancel: tokio::sync::watch::Sender<bool>,
}

async fn cancellable_sync<F>(
    operation: F,
    cancelled: &mut tokio::sync::watch::Receiver<bool>,
) -> Result<i64>
where
    F: std::future::Future<Output = Result<i64>>,
{
    tokio::select! {
        biased;
        result = operation => result,
        _ = cancelled.changed() => Err(anyhow::anyhow!("Sync cancelled; upload outcome may be indeterminate")),
    }
}

fn finish_mcp_sync(
    profile: &mut Profile,
    _path: &Path,
    completed: (VaultConfig, Result<i64>),
) -> Result<Result<i64>> {
    let (vault, result) = completed;
    profile.vault = Some(vault);
    Ok(result)
}

fn start_mcp_sync(profile: &Profile, path: &Path) -> Result<McpSync> {
    let server = profile.server.clone();
    let token = token(profile)?.to_owned();
    let mut vault = profile.vault.clone().context("no vault connected")?;
    let mut journal = crate::state::Journal::open(path, &mut vault)?;
    let (cancel, mut cancelled) = tokio::sync::watch::channel(false);
    let worker = tokio::spawn(async move {
        // Dropping the Sync future cancels network I/O, while the owned vault
        // still carries every completed filesystem transition for persistence.
        let result = cancellable_sync(
            crate::sync::once_with_checkpoint(&server, &token, &mut vault, &mut |state| {
                journal.checkpoint(state)
            }),
            &mut cancelled,
        )
        .await;
        (vault, result)
    });
    Ok(McpSync { worker, cancel })
}

fn mcp_tool_result(id: Value, result: Result<Value>) -> Value {
    match result {
        Ok(value) => {
            json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":value.to_string()}],"isError":false}})
        }
        Err(error) => {
            json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":error.to_string()}],"isError":true}})
        }
    }
}

fn cancellation_responses(
    cancel_id: Value,
    pending_id: Option<Value>,
    result: Result<i64>,
) -> Vec<Value> {
    let completed = result.is_ok();
    let mut responses = Vec::new();
    if let Some(id) = pending_id {
        responses.push(mcp_tool_result(id, result.map(|revision| json!({"applied_revision":revision,"sync":"server_committed_or_applied"}))));
    }
    responses.push(mcp_tool_result(cancel_id, Ok(json!({
        "cancelled": !completed,
        "outcome": if completed { "completed before cancellation" } else { "indeterminate; run sync_run to reconcile" }
    }))));
    responses
}

async fn mcp(mut profile: Profile, path: &Path, auto_sync_seconds: Option<u64>) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    if auto_sync_seconds.is_some_and(|seconds| !(1..=3600).contains(&seconds)) {
        bail!("auto-sync interval must be 1-3600 seconds");
    }
    let mut lines = tokio::io::BufReader::new(tokio::io::stdin());
    let mut pending_line = Vec::new();
    let mut pending_oversized = false;
    let mut stdout = tokio::io::stdout();
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(
        auto_sync_seconds.unwrap_or(3600),
    ));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ticker.tick().await;
    let mut active_sync: Option<McpSync> = None;
    let mut pending_sync_id: Option<Value> = None;
    loop {
        enum Event {
            Line(Option<(Vec<u8>, bool)>),
            Tick,
            SyncDone(std::result::Result<(VaultConfig, Result<i64>), tokio::task::JoinError>),
        }
        let event = tokio::select! {
            line = read_mcp_frame(&mut lines, &mut pending_line, &mut pending_oversized) => Event::Line(line?),
            _ = ticker.tick(), if auto_sync_seconds.is_some() => Event::Tick,
            done = async { (&mut active_sync.as_mut().expect("guarded active Sync").worker).await }, if active_sync.is_some() => Event::SyncDone(done),
        };
        let next = match event {
            Event::Line(line) => line,
            Event::Tick => {
                if active_sync.is_none() && profile.token.is_some() && profile.vault.is_some() {
                    active_sync = Some(start_mcp_sync(&profile, path)?);
                }
                continue;
            }
            Event::SyncDone(done) => {
                active_sync = None;
                let result =
                    finish_mcp_sync(&mut profile, path, done.context("Sync worker failed")?)?;
                if let Some(id) = pending_sync_id.take() {
                    let result = result.map(|revision| json!({"applied_revision":revision,"sync":"server_committed_or_applied"}));
                    let response = mcp_tool_result(id, result);
                    stdout.write_all(response.to_string().as_bytes()).await?;
                    stdout.write_all(b"\n").await?;
                    stdout.flush().await?;
                } else if let Err(error) = result {
                    eprintln!("background Sync failed: {error:#}");
                }
                continue;
            }
        };
        let Some((line, oversized)) = next else {
            break;
        };
        if oversized {
            stdout.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":null,\"error\":{\"code\":-32600,\"message\":\"Request exceeds 2 MiB\"}}\n").await?;
            stdout.flush().await?;
            continue;
        }
        let request: Value = match serde_json::from_slice(&line) {
            Ok(v) => v,
            Err(_) => {
                stdout.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":null,\"error\":{\"code\":-32700,\"message\":\"Parse error\"}}\n").await?;
                stdout.flush().await?;
                continue;
            }
        };
        let id = request.get("id").cloned();
        if !request.is_object()
            || request["jsonrpc"] != "2.0"
            || !request["method"].is_string()
            || id
                .as_ref()
                .is_some_and(|id| !(id.is_string() || id.is_i64() || id.is_u64() || id.is_null()))
        {
            let response = json!({"jsonrpc":"2.0","id":id.filter(|id| id.is_string() || id.is_i64() || id.is_u64()).unwrap_or(Value::Null),
                "error":{"code":-32600,"message":"Invalid Request"}});
            stdout.write_all(response.to_string().as_bytes()).await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
            continue;
        }
        let Some(id) = id else {
            continue;
        };
        let method = request["method"].as_str().unwrap_or("");
        let response = match method {
            "initialize" => json!({"jsonrpc":"2.0","id":id,"result":{
                "protocolVersion":"2025-06-18","capabilities":{"tools":{"listChanged":false}},
                "serverInfo":{"name":"blackglass-headless-native","version":env!("CARGO_PKG_VERSION")}}}),
            "ping" => json!({"jsonrpc":"2.0","id":id,"result":{}}),
            "tools/list" => json!({"jsonrpc":"2.0","id":id,"result":tools()}),
            "tools/call" => {
                let name = request["params"]["name"].as_str().unwrap_or("");
                let args = &request["params"]["arguments"];
                let args = match mcp_call_arguments(name, args) {
                    Ok(value) => value,
                    Err(error) => {
                        let response = json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":error.to_string()}});
                        stdout.write_all(response.to_string().as_bytes()).await?;
                        stdout.write_all(b"\n").await?;
                        stdout.flush().await?;
                        continue;
                    }
                };
                if name == "sync_run" {
                    match (|| {
                        if active_sync.is_some() {
                            bail!("Sync is already running");
                        }
                        start_mcp_sync(&profile, path)
                    })() {
                        Ok(worker) => {
                            active_sync = Some(worker);
                            pending_sync_id = Some(id);
                            continue;
                        }
                        Err(error) => {
                            let response = mcp_tool_result(id, Err(error));
                            stdout.write_all(response.to_string().as_bytes()).await?;
                            stdout.write_all(b"\n").await?;
                            stdout.flush().await?;
                            continue;
                        }
                    }
                }
                if name == "sync_cancel" {
                    let responses = match active_sync.take() {
                        Some(worker) => {
                            let _ = worker.cancel.send(true);
                            let result = finish_mcp_sync(
                                &mut profile,
                                path,
                                worker.worker.await.context("Sync worker failed")?,
                            )?;
                            cancellation_responses(id, pending_sync_id.take(), result)
                        }
                        None => vec![mcp_tool_result(
                            id,
                            Err(anyhow::anyhow!("no Sync is running")),
                        )],
                    };
                    for response in responses {
                        stdout.write_all(response.to_string().as_bytes()).await?;
                        stdout.write_all(b"\n").await?;
                    }
                    stdout.flush().await?;
                    continue;
                }
                let outcome: Result<Value> = (|| {
                    let vault = profile.vault.as_mut().context("no vault connected")?;
                    match name {
                        "note_list" => crate::notes::list(&vault.root),
                        "note_read" => crate::notes::read(
                            &vault.root,
                            args["path"].as_str().context("path required")?,
                        ),
                        "note_search" => crate::notes::search(
                            &vault.root,
                            args["query"].as_str().context("query required")?,
                        ),
                        "note_write" => {
                            if active_sync.is_some() {
                                bail!("Sync is running; retry note_write after it finishes");
                            }
                            crate::notes::write(
                                &vault.root,
                                args["path"].as_str().context("path required")?,
                                args["content"].as_str().context("content required")?,
                                args["expected_sha256"].as_str(),
                            )
                        }
                        "sync_status" => Ok(
                            json!({"root":vault.root,"applied_revision":vault.applied_version,"in_progress":active_sync.is_some()}),
                        ),
                        _ => bail!("unknown tool"),
                    }
                })();
                mcp_tool_result(id, outcome)
            }
            _ => {
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Method not found"}})
            }
        };
        stdout.write_all(response.to_string().as_bytes()).await?;
        stdout.write_all(b"\n").await?;
        stdout.flush().await?;
    }
    if let Some(worker) = active_sync {
        let _ = worker.cancel.send(true);
        let _ = finish_mcp_sync(
            &mut profile,
            path,
            worker
                .worker
                .await
                .context("Sync worker failed at MCP EOF")?,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancellation_persists_multi_revision_progress() {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        }
        let path = dir.path().join("profile.json");
        let mut profile = Profile {
            server: String::new(),
            token: None,
            vault: None,
        };
        let mut vault = VaultConfig {
            id: "v".into(),
            host: String::new(),
            salt: String::new(),
            base_key: String::new(),
            root: dir.path().join("vault"),
            applied_version: 0,
            known: BTreeMap::new(),
        };
        let mut stale = vault.clone();
        let mut journal = crate::state::Journal::open(&path, &mut vault).unwrap();
        let (cancel, mut receiver) = tokio::sync::watch::channel(false);
        let (reached, progress) = tokio::sync::oneshot::channel();
        let worker = tokio::spawn(async move {
            let result = cancellable_sync(
                async {
                    vault.applied_version = 1;
                    vault.known.insert("a.md".into(), "v1".into());
                    journal.checkpoint(&vault)?;
                    assert_eq!(vault.applied_version, 1);
                    vault.applied_version = 2;
                    vault.known.insert("a.md".into(), "v2".into());
                    journal.checkpoint(&vault)?;
                    let _ = reached.send(());
                    std::future::pending::<Result<i64>>().await
                },
                &mut receiver,
            )
            .await;
            (vault, result)
        });
        progress.await.unwrap();
        cancel.send(true).unwrap();
        let result = finish_mcp_sync(&mut profile, &path, worker.await.unwrap()).unwrap();
        assert!(result.is_err());
        crate::state::Journal::open(&path, &mut stale).unwrap();
        assert_eq!(stale.applied_version, 2);
        assert_eq!(stale.known.get("a.md").map(String::as_str), Some("v2"));
    }

    #[tokio::test]
    async fn cancellation_after_completed_worker_keeps_result() {
        let (cancel, mut receiver) = tokio::sync::watch::channel(false);
        let worker =
            tokio::spawn(async move { cancellable_sync(async { Ok(7) }, &mut receiver).await });
        tokio::task::yield_now().await;
        let _ = cancel.send(true);
        assert_eq!(worker.await.unwrap().unwrap(), 7);
    }

    #[test]
    fn cancellation_reports_actual_completed_worker_result() {
        let messages = cancellation_responses(json!(2), Some(json!(1)), Ok(7));
        assert_eq!(messages[0]["id"], 1);
        assert_eq!(messages[0]["result"]["isError"], false);
        assert_eq!(messages[1]["id"], 2);
        assert_eq!(messages[1]["result"]["isError"], false);
        assert!(
            messages[1]["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("\"cancelled\":false")
        );
        let messages =
            cancellation_responses(json!(2), Some(json!(1)), Err(anyhow::anyhow!("cancelled")));
        assert_eq!(messages[0]["result"]["isError"], true);
        assert!(
            messages[1]["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("\"cancelled\":true")
        );
    }

    #[tokio::test]
    async fn oversized_mcp_frame_is_drained_without_allocation() {
        use tokio::io::AsyncWriteExt;
        let (mut writer, reader) = tokio::io::duplex(8192);
        let sender = tokio::spawn(async move {
            let chunk = vec![b'x'; 8192];
            for _ in 0..260 {
                writer.write_all(&chunk).await.unwrap();
            }
            writer
                .write_all(b"\n{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n")
                .await
                .unwrap();
        });
        let mut reader = tokio::io::BufReader::new(reader);
        let mut pending = Vec::new();
        let mut overflow = false;
        let (first, oversized) = read_mcp_frame(&mut reader, &mut pending, &mut overflow)
            .await
            .unwrap()
            .unwrap();
        assert!(oversized);
        assert!(first.len() <= 2 * 1024 * 1024);
        let (second, oversized) = read_mcp_frame(&mut reader, &mut pending, &mut overflow)
            .await
            .unwrap()
            .unwrap();
        assert!(!oversized);
        assert!(serde_json::from_slice::<Value>(&second).is_ok());
        sender.await.unwrap();
    }

    #[tokio::test]
    async fn mcp_frame_survives_select_cancellation() {
        use tokio::io::AsyncWriteExt;
        let (mut writer, reader) = tokio::io::duplex(256);
        let mut reader = tokio::io::BufReader::new(reader);
        let mut pending = Vec::new();
        let mut overflow = false;
        writer.write_all(b"{\"jsonrpc\":\"2.0\",").await.unwrap();
        let interrupted = tokio::time::timeout(
            std::time::Duration::from_millis(10),
            read_mcp_frame(&mut reader, &mut pending, &mut overflow),
        )
        .await;
        assert!(interrupted.is_err());
        assert!(!pending.is_empty());
        writer
            .write_all(b"\"id\":1,\"method\":\"ping\"}\n")
            .await
            .unwrap();
        let (whole, oversized) = read_mcp_frame(&mut reader, &mut pending, &mut overflow)
            .await
            .unwrap()
            .unwrap();
        assert!(!oversized);
        assert_eq!(
            serde_json::from_slice::<Value>(&whole).unwrap()["method"],
            "ping"
        );
    }

    #[test]
    fn mcp_arguments_are_checked_before_mutation() {
        assert!(mcp_call_arguments("note_write", &json!({"path":"x.md","content":42})).is_err());
        assert!(
            mcp_call_arguments(
                "note_write",
                &json!({"path":"x.md","content":"ok","extra":1})
            )
            .is_err()
        );
        assert!(mcp_call_arguments("note_write", &json!({"path":"x.md","content":"ok"})).is_ok());
    }
}
