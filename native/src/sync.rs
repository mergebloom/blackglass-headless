use crate::{app::VaultConfig, crypto::VaultCrypto};
use anyhow::{Context, Result, bail};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::net::TcpStream;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream,
    tungstenite::{Message, client::IntoClientRequest},
};
use url::Url;

const PIECE: usize = 2 * 1024 * 1024;
type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub fn validate_data_host(server: &str, host: &str, expected: Option<&str>) -> Result<()> {
    let control = crate::app::control_origin(server)?;
    if host.is_empty()
        || host.contains('/')
        || host.contains('@')
        || host.contains('?')
        || host.contains('#')
    {
        bail!("invalid data host");
    }
    let data = Url::parse(&format!("{}://{host}/", control.scheme()))?;
    if data.host_str().is_none() || data.username() != "" || data.password().is_some() {
        bail!("invalid data host");
    }
    if data.host_str() != control.host_str() && expected != Some(host) {
        bail!("data host differs from control host; explicitly pin it with --data-host {host}");
    }
    if let Some(expected) = expected
        && expected != host
    {
        bail!("data host did not match explicit pin");
    }
    Ok(())
}

pub fn local_path(root: &Path, relative: &str) -> Result<PathBuf> {
    crate::root::verify(root)?;
    let path = Path::new(relative);
    if relative.is_empty()
        || relative.contains('\\')
        || path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        bail!("unsafe remote path");
    }
    let mut current = root.to_path_buf();
    for component in path.components() {
        current.push(component);
        if let Ok(meta) = fs::symlink_metadata(&current)
            && meta.file_type().is_symlink()
        {
            bail!("symlink in vault path");
        }
    }
    Ok(current)
}

struct Wire {
    socket: Socket,
    queued: NoticeSpool,
}

struct NoticeSpool {
    writer: fs::File,
    reader: BufReader<fs::File>,
    bytes: u64,
    #[cfg(not(unix))]
    path: PathBuf,
}

impl NoticeSpool {
    fn new() -> Result<Self> {
        let mut random = [0u8; 16];
        use rand::RngCore;
        rand::thread_rng().fill_bytes(&mut random);
        let path = std::env::temp_dir().join(format!("blackglass-notice-{}", hex::encode(random)));
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let writer = options.open(&path)?;
        let mut read_options = fs::OpenOptions::new();
        read_options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            read_options.custom_flags(libc::O_NOFOLLOW);
        }
        let reader_file = match read_options.open(&path) {
            Ok(file) => file,
            Err(error) => {
                let _ = fs::remove_file(&path);
                return Err(error.into());
            }
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let writer_meta = writer.metadata()?;
            let reader_meta = reader_file.metadata()?;
            if (writer_meta.dev(), writer_meta.ino()) != (reader_meta.dev(), reader_meta.ino()) {
                let _ = fs::remove_file(&path);
                bail!("notice spool was replaced before reopening");
            }
        }
        let reader = BufReader::new(reader_file);
        #[cfg(unix)]
        if let Err(error) = fs::remove_file(&path) {
            let _ = fs::remove_file(&path);
            return Err(error.into());
        }
        Ok(Self {
            writer,
            reader,
            bytes: 0,
            #[cfg(not(unix))]
            path,
        })
    }

    fn push(&mut self, value: &Value) -> Result<()> {
        let line = serde_json::to_vec(value)?;
        let next = self
            .bytes
            .checked_add(line.len() as u64 + 1)
            .context("notice spool overflow")?;
        if next > 512 * 1024 * 1024 {
            bail!("notice replay exceeds 512 MiB local spool bound");
        }
        self.writer.write_all(&line)?;
        self.writer.write_all(b"\n")?;
        self.bytes = next;
        Ok(())
    }

    fn pop(&mut self) -> Result<Option<Value>> {
        let mut line = String::new();
        if self.reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        Ok(Some(serde_json::from_str(&line)?))
    }
}

impl Drop for NoticeSpool {
    fn drop(&mut self) {
        #[cfg(not(unix))]
        let _ = fs::remove_file(&self.path);
    }
}
impl Wire {
    async fn send(&mut self, value: Value) -> Result<()> {
        let send = self.socket.send(Message::Text(value.to_string().into()));
        tokio::time::timeout(std::time::Duration::from_secs(30), send)
            .await
            .context("Sync send timed out")??;
        Ok(())
    }
    async fn next(&mut self) -> Result<Message> {
        loop {
            let frame =
                tokio::time::timeout(std::time::Duration::from_secs(60), self.socket.next())
                    .await
                    .context("Sync peer timed out")?
                    .context("Sync connection closed")??;
            match frame {
                Message::Ping(bytes) => {
                    tokio::time::timeout(
                        std::time::Duration::from_secs(30),
                        self.socket.send(Message::Pong(bytes)),
                    )
                    .await
                    .context("Sync pong timed out")??;
                }
                Message::Pong(_) => {}
                Message::Close(_) => bail!("Sync connection closed"),
                other => return Ok(other),
            }
        }
    }
    async fn response(&mut self) -> Result<Value> {
        loop {
            let Message::Text(text) = self.next().await? else {
                bail!("unexpected binary response");
            };
            let value: Value = serde_json::from_str(&text)?;
            if value["op"] == "push" {
                self.queued.push(&value)?;
                continue;
            }
            if value["op"] == "pong" {
                continue;
            }
            if value["res"] == "err" || value.get("err").is_some() {
                bail!(
                    "Sync server rejected request: {}",
                    value["msg"]
                        .as_str()
                        .or_else(|| value["err"].as_str())
                        .unwrap_or("unknown")
                );
            }
            return Ok(value);
        }
    }
}

pub fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn current_file_hash(root: &Path, path: &Path) -> Result<Option<String>> {
    match crate::root::open_regular_beneath(root, path.strip_prefix(root)?) {
        Ok(file) => {
            let meta = file.metadata()?;
            if meta.len() > 50 * 1024 * 1024 {
                bail!("local file exceeds 50 MiB limit");
            }
            let mut content = Vec::new();
            file.take(50 * 1024 * 1024 + 1).read_to_end(&mut content)?;
            if content.len() > 50 * 1024 * 1024 {
                bail!("local file exceeds 50 MiB limit");
            }
            Ok(Some(hash(&content)))
        }
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|inner| inner.kind() == std::io::ErrorKind::NotFound) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

pub fn sync_directory_chain(root: &Path, target_parent: &Path) -> Result<()> {
    let relative = target_parent.strip_prefix(root)?;
    crate::root::directory_beneath(root, relative, false)?.sync_all()?;
    Ok(())
}

fn preserve_version(root: &Path, source: &Path) -> Result<()> {
    let history = crate::root::history_dir(root)?;
    let mut random = [0u8; 16];
    use rand::RngCore;
    rand::thread_rng().fill_bytes(&mut random);
    let destination = history.join(hex::encode(random));
    #[cfg(target_os = "linux")]
    {
        use std::{
            ffi::CString,
            os::{
                fd::AsRawFd,
                unix::{ffi::OsStrExt, fs::OpenOptionsExt},
            },
        };
        let source_dir = crate::root::directory_beneath(
            root,
            source
                .parent()
                .context("version has no parent")?
                .strip_prefix(root)?,
            false,
        )?;
        crate::root::history_for_directory(root, &source_dir)?;
        let mut options = fs::OpenOptions::new();
        options
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW);
        let history_dir = options.open(&history)?;
        let from = CString::new(
            source
                .file_name()
                .context("version has no filename")?
                .as_bytes(),
        )?;
        let to = CString::new(
            destination
                .file_name()
                .context("history name missing")?
                .as_bytes(),
        )?;
        if unsafe {
            libc::renameat2(
                source_dir.as_raw_fd(),
                from.as_ptr(),
                history_dir.as_raw_fd(),
                to.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error())
                .context("could not preserve displaced version");
        }
        history_dir.sync_all()?;
        source_dir.sync_all()?;
    }
    #[cfg(not(target_os = "linux"))]
    {
        let source_parent = crate::root::directory_beneath(
            root,
            source
                .parent()
                .context("version has no parent")?
                .strip_prefix(root)?,
            false,
        )?;
        crate::root::history_for_directory(root, &source_parent)?;
        fs::rename(source, &destination)?;
        fs::File::open(&history)?.sync_all()?;
        fs::File::open(source.parent().context("version has no parent")?)?.sync_all()?;
    }
    Ok(())
}

pub fn guarded_replace(
    _root: &Path,
    temp: &Path,
    target: &Path,
    observed: Option<&str>,
) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::{
            ffi::CString,
            os::{fd::AsRawFd, unix::ffi::OsStrExt},
        };
        let parent = crate::root::directory_beneath(
            _root,
            target
                .parent()
                .context("target has no parent")?
                .strip_prefix(_root)?,
            false,
        )?;
        if temp.parent() != target.parent() {
            bail!("replacement crosses directories");
        }
        if observed.is_some() {
            // An exchange precedes moving the displaced version into history.
            // Check the destination filesystem before making that exchange.
            crate::root::history_for_directory(_root, &parent)?;
        }
        let from = CString::new(
            temp.file_name()
                .context("temporary name missing")?
                .as_bytes(),
        )?;
        let to = CString::new(
            target
                .file_name()
                .context("target name missing")?
                .as_bytes(),
        )?;
        let flags = if observed.is_some() {
            libc::RENAME_EXCHANGE
        } else {
            libc::RENAME_NOREPLACE
        };
        let rename = |flags| unsafe {
            libc::renameat2(
                parent.as_raw_fd(),
                from.as_ptr(),
                parent.as_raw_fd(),
                to.as_ptr(),
                flags,
            )
        };
        if rename(flags) != 0 {
            return Err(std::io::Error::last_os_error())
                .context("target changed before atomic replacement");
        }
        if let Some(expected) = observed {
            let displaced = current_file_hash(_root, temp);
            if displaced.as_ref().ok().and_then(|value| value.as_deref()) != Some(expected) {
                if rename(libc::RENAME_EXCHANGE) != 0 {
                    bail!("replacement conflict; both versions retained, inspect partial file");
                }
                bail!(
                    "target changed during atomic replacement; proposed content retained in partial file"
                );
            }
            preserve_version(_root, temp)?;
        }
        return Ok(());
    }
    #[cfg(not(target_os = "linux"))]
    {
        if current_file_hash(_root, target)?.as_deref() != observed {
            bail!("target changed before replacement; partial file retained");
        }
        fs::rename(temp, target)?;
        Ok(())
    }
}

fn install_download(
    root: &Path,
    relative: &str,
    target: &Path,
    observed: Option<&str>,
    plain: &[u8],
) -> Result<()> {
    if local_path(root, relative)? != target
        || current_file_hash(root, target)?.as_deref() != observed
    {
        bail!("local path changed while downloading {relative}; remote revision not applied");
    }
    let parent = target.parent().context("target has no parent")?;
    crate::root::directory_beneath(root, parent.strip_prefix(root)?, true)?.sync_all()?;
    let temp = target.with_extension("blackglass-partial");
    let mut file = crate::root::create_file_beneath(root, temp.strip_prefix(root)?)?;
    file.write_all(plain)?;
    file.sync_all()?;
    if current_file_hash(root, target)?.as_deref() != observed {
        bail!("local path changed before installing {relative}; partial download retained");
    }
    guarded_replace(root, &temp, target, observed)?;
    sync_directory_chain(root, parent)?;
    Ok(())
}

async fn pull(
    wire: &mut Wire,
    uid: i64,
    crypto: &VaultCrypto,
    expected_hash: &str,
) -> Result<Vec<u8>> {
    wire.send(json!({"op":"pull","uid":uid})).await?;
    let header = wire.response().await?;
    if header["res"] != "ok" {
        bail!("unexpected pull response");
    }
    let size = header["size"].as_u64().context("pull size missing")? as usize;
    if size > 55 * 1024 * 1024 {
        bail!("pull exceeds client file bound");
    }
    let mut ciphertext = Vec::with_capacity(size);
    while ciphertext.len() < size {
        let Message::Binary(bytes) = wire.next().await? else {
            bail!("expected pull data");
        };
        if bytes.len() > PIECE || ciphertext.len() + bytes.len() > size {
            bail!("invalid pull piece");
        }
        ciphertext.extend_from_slice(&bytes);
    }
    let plaintext = if size == 0 {
        Vec::new()
    } else {
        crypto.decrypt_body(&ciphertext)?
    };
    if hash(&plaintext) != expected_hash {
        bail!("remote content hash mismatch");
    }
    Ok(plaintext)
}

async fn apply_notice(
    wire: &mut Wire,
    vault: &mut VaultConfig,
    crypto: &VaultCrypto,
    notice: Value,
) -> Result<()> {
    let uid = notice["uid"]
        .as_i64()
        .context("push notice missing revision")?;
    if uid <= vault.applied_version {
        return Ok(());
    }
    let encoded = notice["path"]
        .as_str()
        .context("push notice missing path")?;
    let relative = crypto.decode_path(encoded)?;
    let target = local_path(&vault.root, &relative)?;
    let deleted = notice["deleted"] == true;
    let folder = notice["folder"] == true;
    if !deleted && !folder {
        let incoming_hash =
            crypto.decode_path(notice["hash"].as_str().context("missing encrypted hash")?)?;
        if current_file_hash(&vault.root, &target)?.as_deref() == Some(incoming_hash.as_str()) {
            crate::root::open_regular_beneath(&vault.root, Path::new(&relative))?.sync_all()?;
            sync_directory_chain(
                &vault.root,
                target.parent().context("target has no parent")?,
            )?;
            if current_file_hash(&vault.root, &target)?.as_deref() != Some(incoming_hash.as_str()) {
                bail!("local file changed while confirming remote revision");
            }
            vault.known.insert(relative, incoming_hash);
            vault.applied_version = uid;
            return Ok(());
        }
    }
    let before = if folder {
        None
    } else {
        current_file_hash(&vault.root, &target)?
    };
    if let Some(known) = vault.known.get(&relative) {
        if before.as_deref() != Some(known) && !(deleted && before.is_none()) {
            bail!("local edit or deletion conflicts with incoming revision at {relative}");
        }
    } else if before.is_some() && !folder {
        bail!("untracked local path conflicts with incoming revision at {relative}");
    }
    if deleted {
        if before.is_some() {
            preserve_version(&vault.root, &target)?;
            sync_directory_chain(
                &vault.root,
                target.parent().context("target has no parent")?,
            )?;
        }
        vault.known.remove(&relative);
    } else if folder {
        crate::root::directory_beneath(&vault.root, target.strip_prefix(&vault.root)?, true)?
            .sync_all()?;
    } else {
        let expected_hash =
            crypto.decode_path(notice["hash"].as_str().context("missing encrypted hash")?)?;
        let plain = pull(wire, uid, crypto, &expected_hash).await?;
        install_download(&vault.root, &relative, &target, before.as_deref(), &plain)?;
        vault.known.insert(relative, expected_hash);
    }
    vault.applied_version = uid;
    Ok(())
}

pub fn inventory(root: &Path) -> Result<BTreeMap<String, PathBuf>> {
    inventory_limited(root, 100_000)
}

#[cfg(target_os = "linux")]
pub fn inventory_limited(root: &Path, limit: usize) -> Result<BTreeMap<String, PathBuf>> {
    use std::os::fd::AsRawFd;
    let mut files = BTreeMap::new();
    let mut stack = vec![(
        String::new(),
        crate::root::directory_beneath(root, Path::new(""), false)?,
        0usize,
    )];
    let mut visited = 0usize;
    while let Some((prefix, directory, depth)) = stack.pop() {
        if depth > 64 {
            bail!("vault exceeds 64 directory levels");
        }
        let descriptor_path = format!("/proc/self/fd/{}", directory.as_raw_fd());
        for entry in fs::read_dir(descriptor_path)? {
            let entry = entry?;
            visited += 1;
            if visited > limit {
                bail!("vault exceeds {limit} filesystem entries");
            }
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| anyhow::anyhow!("non-UTF-8 vault path"))?;
            let relative = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                bail!("symlink in local vault: {relative}");
            }
            if kind.is_dir() {
                let child = crate::root::directory_beneath(root, Path::new(&relative), false)?;
                stack.push((relative, child, depth + 1));
            } else if kind.is_file() {
                if relative.ends_with(".blackglass-tmp")
                    || relative.ends_with(".blackglass-partial")
                {
                    bail!("incomplete local operation: {relative}");
                }
                let _ = crate::root::open_regular_beneath(root, Path::new(&relative))?;
                files.insert(relative.clone(), root.join(relative));
            } else {
                bail!("special file in local vault: {relative}");
            }
        }
    }
    Ok(files)
}

#[cfg(not(target_os = "linux"))]
pub fn inventory_limited(root: &Path, limit: usize) -> Result<BTreeMap<String, PathBuf>> {
    crate::root::verify(root)?;
    let mut files = BTreeMap::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    let mut visited = 0usize;
    while let Some((dir, depth)) = stack.pop() {
        crate::root::verify(root)?;
        if depth > 64 {
            bail!("vault exceeds 64 directory levels");
        }
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            visited += 1;
            if visited > limit {
                bail!("vault exceeds {limit} filesystem entries");
            }
            let path = entry.path();
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                bail!("symlink in local vault: {}", path.display());
            }
            if kind.is_dir() {
                stack.push((path, depth + 1));
            } else if kind.is_file() {
                let relative = path
                    .strip_prefix(root)?
                    .to_str()
                    .context("non-UTF-8 vault path")?
                    .to_owned();
                if relative.ends_with(".blackglass-tmp")
                    || relative.ends_with(".blackglass-partial")
                {
                    bail!("incomplete local operation: {}", path.display());
                }
                local_path(root, &relative)?;
                files.insert(relative, path);
            } else {
                bail!("special file in local vault: {}", path.display());
            }
        }
    }
    Ok(files)
}

async fn push(
    wire: &mut Wire,
    crypto: &VaultCrypto,
    relative: &str,
    content: Option<&[u8]>,
) -> Result<()> {
    let deleted = content.is_none();
    let bytes = content.unwrap_or_default();
    if bytes.len() > 50 * 1024 * 1024 {
        bail!("local file exceeds 50 MiB limit: {relative}");
    }
    let encrypted = if bytes.is_empty() {
        Vec::new()
    } else {
        crypto.encrypt_body(bytes)?
    };
    let millis = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as i64;
    let extension = Path::new(relative)
        .extension()
        .and_then(|x| x.to_str())
        .unwrap_or("");
    wire.send(json!({"op":"push","path":crypto.encode_path(relative)?,"relatedpath":null,
        "extension":extension,"hash":if deleted { String::new() } else { crypto.content_hash(bytes)? },
        "ctime":millis,"mtime":millis,"folder":false,"deleted":deleted,
        "size":encrypted.len(),"pieces":encrypted.len().div_ceil(PIECE)})).await?;
    if encrypted.is_empty() {
        let ack = wire
            .response()
            .await
            .context("empty upload outcome indeterminate; reconcile before retry")?;
        if ack["res"] != "ok" {
            bail!("unexpected push acknowledgement");
        }
        return Ok(());
    }
    let ready = wire
        .response()
        .await
        .context("upload outcome indeterminate; reconcile before retry")?;
    if ready["res"] != "next" {
        bail!("unexpected push readiness response");
    }
    for (index, piece) in encrypted.chunks(PIECE).enumerate() {
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            wire.socket.send(Message::Binary(piece.to_vec().into())),
        )
        .await
        .context("Sync upload send timed out")??;
        let ack = wire
            .response()
            .await
            .context("upload outcome indeterminate; reconcile before retry")?;
        let final_piece = index + 1 == encrypted.len().div_ceil(PIECE);
        if ack["res"] != if final_piece { "ok" } else { "next" } {
            bail!("indeterminate upload acknowledgement for {relative}");
        }
    }
    Ok(())
}

pub async fn once(server: &str, token: &str, vault: &mut VaultConfig) -> Result<i64> {
    tokio::time::timeout(
        std::time::Duration::from_secs(15 * 60),
        once_inner(server, token, vault),
    )
    .await
    .context("Sync exceeded 15-minute operation deadline")?
}

async fn once_inner(server: &str, token: &str, vault: &mut VaultConfig) -> Result<i64> {
    validate_data_host(server, &vault.host, Some(&vault.host))?;
    let base = hex::decode(&vault.base_key)?;
    let base: [u8; 32] = base
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid local key"))?;
    let crypto = VaultCrypto::from_base_key(&base, &vault.salt)?;
    let scheme = if server.starts_with("https:") {
        "wss"
    } else {
        "ws"
    };
    let url = format!("{scheme}://{}/", vault.host);
    let mut request = url.into_client_request()?;
    request
        .headers_mut()
        .insert("Origin", "app://obsidian.md".parse()?);
    let (socket, _) = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        tokio_tungstenite::connect_async(request),
    )
    .await
    .context("Sync connection timed out")??;
    let mut wire = Wire {
        socket,
        queued: NoticeSpool::new()?,
    };
    wire.send(json!({"op":"init","token":token,"id":vault.id,"keyhash":crypto.key_hash(),
        "version":vault.applied_version,"initial":vault.applied_version==0,"device":"Blackglass Headless Native","encryption_version":3})).await?;
    let init = wire.response().await?;
    if init["res"] != "ok" {
        bail!("unexpected Sync init response");
    }
    let ready = wire.response().await?;
    if ready["op"] != "ready" {
        bail!("unexpected Sync bootstrap message");
    }
    let boundary = ready["version"].as_i64().context("ready version missing")?;
    while let Some(notice) = wire.queued.pop()? {
        apply_notice(&mut wire, vault, &crypto, notice).await?;
    }
    vault.applied_version = vault.applied_version.max(boundary);
    let files = inventory(&vault.root)?;
    for relative in files.keys() {
        let file = crate::root::open_regular_beneath(&vault.root, Path::new(relative))?;
        if file.metadata()?.len() > 50 * 1024 * 1024 {
            bail!("local file exceeds 50 MiB limit: {relative}");
        }
        let mut content = Vec::new();
        file.take(50 * 1024 * 1024 + 1).read_to_end(&mut content)?;
        if content.len() > 50 * 1024 * 1024 {
            bail!("local file exceeds 50 MiB limit: {relative}");
        }
        let digest = hash(&content);
        if vault.known.get(relative) == Some(&digest) {
            continue;
        }
        // The server has no conditional-write or idempotency fields. A missing
        // acknowledgement is indeterminate and is never reported as success.
        push(&mut wire, &crypto, relative, Some(&content)).await?;
        vault.known.insert(relative.clone(), digest);
    }
    let missing: Vec<String> = vault
        .known
        .keys()
        .filter(|name| !files.contains_key(*name))
        .cloned()
        .collect();
    for relative in missing {
        push(&mut wire, &crypto, &relative, None).await?;
        vault.known.remove(&relative);
    }
    while let Some(notice) = wire.queued.pop()? {
        apply_notice(&mut wire, vault, &crypto, notice).await?;
    }
    Ok(vault.applied_version)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_unsafe_paths_and_hosts() {
        for path in ["../escape", "/absolute", "", "a/../b"] {
            assert!(local_path(Path::new("/tmp/vault"), path).is_err());
        }
        assert!(
            validate_data_host(
                "https://notes.example.test/",
                "elsewhere.example.test",
                None
            )
            .is_err()
        );
        assert!(
            validate_data_host(
                "https://notes.example.test/",
                "elsewhere.example.test",
                Some("elsewhere.example.test")
            )
            .is_ok()
        );
    }

    #[test]
    fn replay_spool_exceeds_old_memory_queue() {
        let mut spool = NoticeSpool::new().unwrap();
        for uid in 1..=10_001 {
            spool.push(&json!({"op":"push","uid":uid})).unwrap();
        }
        for uid in 1..=10_001 {
            assert_eq!(spool.pop().unwrap().unwrap()["uid"], uid);
        }
        assert!(spool.pop().unwrap().is_none());
        spool.push(&json!({"op":"push","uid":10_002})).unwrap();
        assert_eq!(spool.pop().unwrap().unwrap()["uid"], 10_002);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_replaced_symlink_root() {
        use std::os::unix::fs::symlink;
        let parent = tempfile::tempdir().unwrap();
        let outside = parent.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let root = parent.path().join("vault");
        symlink(&outside, &root).unwrap();
        assert!(local_path(&root, "note.md").is_err());
        assert!(inventory(&root).is_err());
    }

    #[test]
    fn download_rejects_edit_or_creation_after_observation() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("note.md");
        fs::write(&target, b"original").unwrap();
        let observed = hash(b"original");
        fs::write(&target, b"new local edit").unwrap();
        assert!(
            install_download(dir.path(), "note.md", &target, Some(&observed), b"remote").is_err()
        );
        assert_eq!(fs::read(&target).unwrap(), b"new local edit");

        let fresh = dir.path().join("fresh.md");
        fs::write(&fresh, b"locally created").unwrap();
        assert!(install_download(dir.path(), "fresh.md", &fresh, None, b"remote").is_err());
        assert_eq!(fs::read(&fresh).unwrap(), b"locally created");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn atomic_replace_preserves_racing_local_edit() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("note.md");
        let temp = dir.path().join("note.blackglass-partial");
        fs::write(&target, b"original").unwrap();
        fs::write(&temp, b"remote").unwrap();
        let observed = hash(b"original");
        fs::write(&target, b"racing local edit").unwrap();
        assert!(guarded_replace(dir.path(), &temp, &target, Some(&observed)).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"racing local edit");
        assert_eq!(fs::read(&temp).unwrap(), b"remote");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn displaced_inode_remains_recoverable_for_open_editor() {
        use std::io::Seek;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("note.md");
        let temp = dir.path().join("note.blackglass-partial");
        fs::write(&target, b"original").unwrap();
        fs::write(&temp, b"remote").unwrap();
        let mut editor = fs::OpenOptions::new().write(true).open(&target).unwrap();
        guarded_replace(dir.path(), &temp, &target, Some(&hash(b"original"))).unwrap();
        editor.seek(std::io::SeekFrom::End(0)).unwrap();
        editor.write_all(b"-late-edit").unwrap();
        editor.sync_all().unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"remote");
        let history = crate::root::history_dir(dir.path()).unwrap();
        let saved: Vec<_> = fs::read_dir(history).unwrap().collect();
        assert_eq!(saved.len(), 1);
        assert_eq!(
            fs::read(saved[0].as_ref().unwrap().path()).unwrap(),
            b"original-late-edit"
        );
    }
}
