use crate::sync::{guarded_replace, hash, inventory_limited, local_path, sync_directory_chain};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    path::Path,
};

const MAX_NOTE: usize = 1024 * 1024;

fn markdown(root: &Path, relative: &str) -> Result<std::path::PathBuf> {
    if !relative.ends_with(".md") {
        bail!("note path must end in .md");
    }
    local_path(root, relative)
}
pub fn list(root: &Path) -> Result<Value> {
    let names: Vec<_> = inventory_limited(root, 10_000)?
        .into_keys()
        .filter(|x| x.ends_with(".md"))
        .take(1000)
        .collect();
    Ok(json!({"notes":names,"truncated":names.len()==1000}))
}
pub fn read(root: &Path, relative: &str) -> Result<Value> {
    markdown(root, relative)?;
    let file = crate::root::open_regular_beneath(root, Path::new(relative))?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.len() as usize > MAX_NOTE {
        bail!("note is not a bounded regular file");
    }
    let mut bytes = Vec::new();
    file.take(MAX_NOTE as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_NOTE {
        bail!("note exceeds 1 MiB tool limit");
    }
    let content = String::from_utf8(bytes.clone()).context("note is not UTF-8")?;
    Ok(json!({"path":relative,"content":content,"sha256":hash(&bytes),"local_only":true}))
}
pub fn search(root: &Path, query: &str) -> Result<Value> {
    if query.trim().is_empty() || query.len() > 100 {
        bail!("query must be 1-100 bytes");
    }
    let needle = query.to_lowercase();
    let mut matches = Vec::new();
    let mut scanned = 0usize;
    for (name, _) in inventory_limited(root, 10_000)?
        .into_iter()
        .filter(|(name, _)| name.ends_with(".md"))
    {
        if scanned >= 1000 || matches.len() >= 20 {
            break;
        }
        scanned += 1;
        let file = crate::root::open_regular_beneath(root, Path::new(&name))?;
        if file.metadata()?.len() as usize > MAX_NOTE {
            continue;
        }
        let mut bytes = Vec::new();
        if file
            .take(MAX_NOTE as u64 + 1)
            .read_to_end(&mut bytes)
            .is_err()
            || bytes.len() > MAX_NOTE
        {
            continue;
        }
        let content = match String::from_utf8(bytes) {
            Ok(value) => value,
            Err(_) => continue,
        };
        if let Some((index, line)) = content
            .lines()
            .enumerate()
            .find(|(_, line)| line.to_lowercase().contains(&needle))
        {
            matches.push(json!({"path":name,"line":index+1,"snippet":line.chars().take(240).collect::<String>()}));
        }
    }
    Ok(
        json!({"matches":matches,"scanned":scanned,"truncated":scanned==1000 || matches.len()==20,"local_only":true}),
    )
}
pub fn write(root: &Path, relative: &str, content: &str, expected: Option<&str>) -> Result<Value> {
    if content.len() > MAX_NOTE {
        bail!("note exceeds 1 MiB tool limit");
    }
    let path = markdown(root, relative)?;
    let observed = if path.exists() {
        let file = crate::root::open_regular_beneath(root, Path::new(relative))?;
        if file.metadata()?.len() > MAX_NOTE as u64 {
            bail!("existing note exceeds 1 MiB tool limit");
        }
        let mut current = Vec::new();
        file.take(MAX_NOTE as u64 + 1).read_to_end(&mut current)?;
        if current.len() > MAX_NOTE {
            bail!("existing note exceeds 1 MiB tool limit");
        }
        let observed = hash(&current);
        if expected != Some(observed.as_str()) {
            bail!("stale or missing expected_sha256");
        }
        Some(observed)
    } else if expected.is_some() {
        bail!("expected existing note but path is absent");
    } else {
        None
    };
    let parent = path.parent().context("note has no parent")?;
    crate::root::directory_beneath(root, parent.strip_prefix(root)?, true)?.sync_all()?;
    let temp = path.with_extension("md.blackglass-tmp");
    let mut file = crate::root::create_file_beneath(root, temp.strip_prefix(root)?)?;
    file.write_all(content.as_bytes())?;
    file.sync_all()?;
    if local_path(root, relative)? != path {
        bail!("note path changed before installation; partial file retained");
    }
    guarded_replace(root, &temp, &path, observed.as_deref())?;
    sync_directory_chain(root, parent)?;
    Ok(
        json!({"path":relative,"sha256":hash(content.as_bytes()),"local_only":true,"sync":"pending"}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expected_hash_blocks_stale_write() {
        let dir = tempfile::tempdir().unwrap();
        let first = write(dir.path(), "n.md", "one", None).unwrap();
        assert!(write(dir.path(), "n.md", "bad", None).is_err());
        assert!(write(dir.path(), "n.md", "bad", Some("wrong")).is_err());
        write(dir.path(), "n.md", "two", first["sha256"].as_str()).unwrap();
        assert_eq!(read(dir.path(), "n.md").unwrap()["content"], "two");
    }
}
