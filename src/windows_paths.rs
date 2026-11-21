use anyhow::{Context, Result, ensure};
use std::path::{Component, Path, PathBuf, Prefix};

pub fn validate(path: &Path, needs_filename: bool) -> Result<()> {
    let text = path.to_str().context("path contains invalid Unicode")?;
    ensure!(!text.is_empty(), "path is empty");
    let relative = match path.components().next() {
        Some(Component::Prefix(prefix)) => {
            ensure!(
                matches!(prefix.kind(), Prefix::Disk(_)) && path.has_root(),
                "use a drive-qualified local path or a relative path"
            );
            &text[2..]
        }
        Some(Component::RootDir) => {
            anyhow::bail!("rooted paths must include a drive letter")
        }
        _ => text,
    };

    let parts = relative.split(['/', '\\']);
    if needs_filename {
        let last = parts
            .clone()
            .next_back()
            .context("missing output filename")?;
        ensure!(
            !matches!(last, "" | "." | ".."),
            "output path must name a file"
        );
    }
    for part in parts.filter(|part| !matches!(*part, "" | "." | "..")) {
        validate_component(part)?;
    }
    Ok(())
}

pub fn validate_component(part: &str) -> Result<()> {
    ensure!(
        !part.is_empty()
            && !part.starts_with(' ')
            && !part.ends_with([' ', '.'])
            && !part
                .chars()
                .any(|ch| ch.is_control() || "<>:\"/\\|?*".contains(ch)),
        "path contains an invalid Windows name"
    );
    // DOS device names are reserved even when followed by an extension.
    let stem = part
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(' ');
    let stem = stem.to_ascii_uppercase();
    let numbered_device = stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"))
        .is_some_and(|suffix| {
            matches!(
                suffix,
                "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        });
    ensure!(
        !numbered_device
            && !matches!(
                stem.as_str(),
                "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
            ),
        "path contains a reserved Windows device name"
    );
    Ok(())
}

pub fn canonicalize_local(path: &Path) -> Result<PathBuf> {
    let resolved = path
        .canonicalize()
        .with_context(|| format!("resolving {}", display(path)))?;
    ensure!(
        matches!(
            resolved.components().next(),
            Some(Component::Prefix(prefix))
                if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_))
        ),
        "path must resolve to a local drive"
    );
    Ok(resolved)
}

/// Format local drive paths for people while retaining canonical paths for I/O.
pub fn display(path: &Path) -> impl std::fmt::Display + '_ {
    std::fmt::from_fn(move |formatter| {
        let text = path.to_string_lossy();
        if matches!(path.components().next(),
            Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::VerbatimDisk(_)))
            && let Some(normal) = text.strip_prefix(r"\\?\")
        {
            return formatter.write_str(normal);
        }
        formatter.write_str(&text)
    })
}

#[cfg(test)]
mod tests {
    use super::{Path, validate};

    #[test]
    fn rejects_special_windows_paths_and_directory_shaped_output() {
        for path in [
            r"\\.\NUL",
            r"\\?\C:\file",
            r"\\server\share\file",
            r"C:file",
            r"\file",
            "archive.kfx-zip:stream",
            "archive.kfx-zip/",
            "archive.kfx-zip/.",
            "archive.kfx-zip.",
            "archive.kfx-zip ",
            "nul.txt",
            "LPT².txt",
        ] {
            assert!(validate(Path::new(path), true).is_err(), "{path}");
        }
        assert!(validate(Path::new(r"C:\My Books\café.kfx-zip"), true).is_ok());
        assert!(validate(Path::new("../exports/book.kfx-zip"), true).is_ok());
        assert!(validate(Path::new("./My Books/"), false).is_ok());
    }
}
