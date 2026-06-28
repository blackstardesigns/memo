use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;

use crate::note::{self, Folder, Note};

/// Name of the JSON index holding the user's folders, stored in the data dir
/// alongside the note files (which are matched only by their `.md` extension,
/// so this file is ignored by [`Store::list`]).
const FOLDERS_FILE: &str = "folders.json";

/// File-backed note store rooted at a single directory.
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(&dir)
            .with_context(|| format!("creating data directory {}", dir.display()))?;
        Ok(Store { dir })
    }

    fn note_path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.md"))
    }

    fn refined_path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.refined.md"))
    }

    /// Load all notes, newest-modified first. Unparseable files are skipped.
    pub fn list(&self) -> Result<Vec<Note>> {
        let mut notes = Vec::new();
        for entry in
            fs::read_dir(&self.dir).with_context(|| format!("reading {}", self.dir.display()))?
        {
            let path = entry?.path();
            let name = match path.file_name().and_then(|s| s.to_str()) {
                Some(n) => n,
                None => continue,
            };
            if !name.ends_with(".md") || name.ends_with(".refined.md") {
                continue;
            }
            let raw = match fs::read_to_string(&path) {
                Ok(s) => s.replace("\r\n", "\n"),
                Err(_) => continue,
            };
            let (meta, content) = match note::parse(&raw) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let refined = {
                let rp = self.refined_path(&meta.id);
                if rp.exists() {
                    fs::read_to_string(&rp)
                        .ok()
                        .map(|s| s.replace("\r\n", "\n"))
                } else {
                    None
                }
            };
            notes.push(Note {
                meta,
                content,
                refined,
            });
        }
        notes.sort_by_key(|n| std::cmp::Reverse(n.meta.modified));
        Ok(notes)
    }

    /// Persist a note, stamping `modified` to now. Writes the refined sidecar if present.
    pub fn save(&self, note: &mut Note) -> Result<()> {
        note.meta.modified = Utc::now();
        fs::write(self.note_path(&note.meta.id), note.to_file_string()?)
            .with_context(|| format!("saving note {}", note.meta.id))?;
        if let Some(refined) = &note.refined {
            fs::write(self.refined_path(&note.meta.id), refined)?;
        }
        Ok(())
    }

    /// Delete a note and its refined sidecar.
    pub fn delete(&self, id: &str) -> Result<()> {
        let _ = fs::remove_file(self.note_path(id));
        let _ = fs::remove_file(self.refined_path(id));
        Ok(())
    }

    fn folders_path(&self) -> PathBuf {
        self.dir.join(FOLDERS_FILE)
    }

    /// Load the folder index. A missing file means "no folders yet".
    ///
    /// If the file exists but can't be parsed, it's preserved as `<name>.bak`
    /// (and an error is returned) rather than left in place to be silently
    /// overwritten by the next [`Store::save_folders`], which would lose the
    /// user's folder structure.
    pub fn list_folders(&self) -> Result<Vec<Folder>> {
        let path = self.folders_path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let raw =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        match serde_json::from_str::<Vec<Folder>>(&raw) {
            Ok(folders) => Ok(folders),
            Err(e) => {
                let backup = self.dir.join(format!("{FOLDERS_FILE}.bak"));
                let _ = fs::rename(&path, &backup);
                Err(anyhow::Error::new(e).context(format!(
                    "{} was not valid JSON; backed it up to {}",
                    path.display(),
                    backup.display()
                )))
            }
        }
    }

    /// Persist the folder index, overwriting it with `folders`.
    pub fn save_folders(&self, folders: &[Folder]) -> Result<()> {
        let json = serde_json::to_string_pretty(folders).context("serializing folders")?;
        fs::write(self.folders_path(), json)
            .with_context(|| format!("writing {}", self.folders_path().display()))?;
        Ok(())
    }
}

/// Export a note's body to a `.md` file at `dest`, choosing the refined or original version.
/// Frontmatter is intentionally omitted from exports.
pub fn export(note: &Note, use_refined: bool, dest: &Path) -> Result<()> {
    let body = if use_refined {
        note.refined.as_deref().unwrap_or(&note.content)
    } else {
        &note.content
    };
    if let Some(parent) = dest.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
    }
    fs::write(dest, body).with_context(|| format!("writing {}", dest.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_list_delete_roundtrip() {
        let dir = std::env::temp_dir().join(format!("ant-test-{}", uuid::Uuid::new_v4()));
        let store = Store::new(dir.clone()).unwrap();

        let mut n = Note::new();
        n.meta.title = "First".into();
        n.content = "hello".into();
        store.save(&mut n).unwrap();

        let loaded = store.list().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].meta.title, "First");
        assert_eq!(loaded[0].content, "hello");
        assert!(loaded[0].refined.is_none());

        n.refined = Some("refined!".into());
        store.save(&mut n).unwrap();
        let loaded = store.list().unwrap();
        assert_eq!(loaded[0].refined.as_deref(), Some("refined!"));

        store.delete(&n.meta.id).unwrap();
        assert_eq!(store.list().unwrap().len(), 0);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn folders_roundtrip_and_default_empty() {
        let dir = std::env::temp_dir().join(format!("ant-folders-{}", uuid::Uuid::new_v4()));
        let store = Store::new(dir.clone()).unwrap();

        // No index file yet => no folders, and note listing ignores folders.json.
        assert!(store.list_folders().unwrap().is_empty());

        let folders = vec![
            crate::note::Folder::new("Work".into()),
            crate::note::Folder::new("Personal".into()),
        ];
        store.save_folders(&folders).unwrap();

        let loaded = store.list_folders().unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].title, "Work");
        assert_eq!(loaded[1].id, folders[1].id);

        // A note filed into a folder roundtrips its `folder` id.
        let mut n = Note::new();
        n.meta.folder = Some(folders[0].id.clone());
        store.save(&mut n).unwrap();
        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(
            listed[0].meta.folder.as_deref(),
            Some(folders[0].id.as_str())
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn corrupt_folders_index_is_backed_up_not_clobbered() {
        let dir = std::env::temp_dir().join(format!("ant-folders-bad-{}", uuid::Uuid::new_v4()));
        let store = Store::new(dir.clone()).unwrap();

        let corrupt = "{ this is not valid json";
        fs::write(dir.join("folders.json"), corrupt).unwrap();

        // Reading errors out (so the app surfaces it) instead of returning empty…
        let err = store.list_folders().unwrap_err();
        assert!(format!("{err:#}").contains("backed it up"));

        // …and the original content is preserved in the .bak, not destroyed.
        let bak = dir.join("folders.json.bak");
        assert!(bak.exists());
        assert_eq!(fs::read_to_string(&bak).unwrap(), corrupt);
        assert!(!dir.join("folders.json").exists());

        let _ = fs::remove_dir_all(dir);
    }
}
