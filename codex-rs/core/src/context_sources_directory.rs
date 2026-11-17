//! Directory sources contain bounded name trees, never file bodies or nested references.

use std::sync::Arc;

use codex_file_system::EnvironmentAccess;
use codex_file_system::GetMetadataOptions;
use codex_utils_path_uri::PathUri;
use futures::StreamExt;
use ignore::gitignore::Gitignore;
use ignore::gitignore::GitignoreBuilder;

const MAX_ENTRIES: usize = 256;
const MAX_SCAN: usize = 4096;
const MAX_BYTES: usize = 16 << 10;
const MAX_IGNORE_BYTES: usize = 64 << 10;

pub(super) async fn read(access: &dyn EnvironmentAccess, root: &PathUri) -> Result<String, String> {
    let root = access.canonicalize(root).await.map_err(|e| e.to_string())?;
    let mut ancestors = vec![root.clone()];
    while let Some(directory) = ancestors.last() {
        let git = directory.join(".git").map_err(|e| e.to_string())?;
        match access
            .get_metadata(&git, GetMetadataOptions::default())
            .await
        {
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
        let Some(parent) = directory.parent() else {
            break;
        };
        if ancestors.len() == 64 {
            return Err("directory ignore ancestry exceeds 64 levels".to_string());
        }
        ancestors.push(parent);
    }
    let mut ignore_bytes = 0;
    let mut ignores = Vec::new();
    for directory in ancestors.into_iter().rev() {
        if let Some(ignore) = load_ignore(access, &directory, &mut ignore_bytes).await? {
            ignores.push(ignore);
        }
    }
    let mut tree = "Directory tree (names only, depth 2):\n./\n".to_string();
    let mut pending = vec![(root, 0, Arc::new(ignores))];
    let mut listed = 0;
    let mut scanned = 0;
    let result: Result<(), String> = async {
        while let Some((path, depth, ignores)) = pending.pop() {
            if depth != 0 {
                let name = path.basename().ok_or("invalid directory entry name")?;
                if matches!(
                    name.as_str(),
                    ".git" | ".svn" | ".hg" | ".bzr" | "_darcs" | "CVS"
                ) {
                    continue;
                }
                let metadata = access
                    .get_metadata(
                        &path,
                        GetMetadataOptions {
                            follow_symlinks: false,
                        },
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                let mut ignored = false;
                for ignore in ignores.iter() {
                    match ignore
                        .matched_path_or_any_parents(path.to_path_buf(), metadata.is_directory)
                    {
                        ignore::Match::Ignore(_) => ignored = true,
                        ignore::Match::Whitelist(_) => ignored = false,
                        ignore::Match::None => {}
                    }
                }
                if ignored {
                    continue;
                }
                let name = if name.chars().any(char::is_control) || name.contains(['"', '\\']) {
                    format!("{name:?}")
                } else {
                    name
                };
                let suffix = if metadata.is_symlink {
                    " (symlink)"
                } else if metadata.is_directory {
                    "/"
                } else {
                    ""
                };
                let line = format!("{}- {name}{suffix}\n", "  ".repeat(depth - 1));
                if listed == MAX_ENTRIES || tree.len() + line.len() > MAX_BYTES - 256 {
                    return Err("listing limit of 256 entries or 16384 bytes".to_string());
                }
                tree.push_str(&line);
                listed += 1;
                if !metadata.is_directory || metadata.is_symlink || depth == 2 {
                    continue;
                }
            }
            let mut next_ignores = ignores.as_ref().clone();
            if depth != 0
                && let Some(ignore) = load_ignore(access, &path, &mut ignore_bytes).await?
            {
                next_ignores.push(ignore);
            }
            // Native access returns one directory's names together. Bound further sorting,
            // metadata reads and traversal before processing that listing.
            let mut entries = access
                .read_directory(&path)
                .await
                .map_err(|e| e.to_string())?;
            if entries.len() > MAX_SCAN - scanned {
                return Err("scan limit of 4096 entries".to_string());
            }
            scanned += entries.len();
            entries.sort_by(|a, b| b.file_name.cmp(&a.file_name));
            let next_ignores = Arc::new(next_ignores);
            for entry in entries {
                pending.push((
                    path.join(&entry.file_name).map_err(|e| e.to_string())?,
                    depth + 1,
                    Arc::clone(&next_ignores),
                ));
            }
        }
        Ok(())
    }
    .await;
    if let Err(error) = result {
        let reason: String = error.chars().take(160).collect();
        tree.push_str(&format!(
            "[TRUNCATED: {reason:?}; remaining entries not listed.]\n"
        ));
    } else if listed == 0 {
        tree.push_str("(empty after .gitignore and VCS metadata exclusions)\n");
    }
    Ok(tree)
}

async fn load_ignore(
    access: &dyn EnvironmentAccess,
    directory: &PathUri,
    total_bytes: &mut usize,
) -> Result<Option<Gitignore>, String> {
    let path = directory.join(".gitignore").map_err(|e| e.to_string())?;
    let mut stream = match access.read_file_stream(&path).await {
        Ok(stream) => stream,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        *total_bytes = total_bytes.saturating_add(chunk.len());
        if *total_bytes > MAX_IGNORE_BYTES {
            return Err("directory ignore rules exceed 65536-byte limit".to_string());
        }
        bytes.extend_from_slice(&chunk);
    }
    let text = String::from_utf8(bytes).map_err(|e| e.to_string())?;
    let mut builder = GitignoreBuilder::new(directory.to_path_buf());
    for line in text.lines() {
        builder
            .add_line(Some(path.to_path_buf()), line)
            .map_err(|e| e.to_string())?;
    }
    builder.build().map(Some).map_err(|e| e.to_string())
}
