use anyhow::{Context, Result, ensure};
use std::{
    ffi::OsString,
    io::{self, BufRead, IsTerminal, Write},
    path::{Path, PathBuf},
};

pub fn has_dedicated_console() -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Console::GetConsoleProcessList;

        let mut process_id = 0;
        // A single attached process suggests a fresh console, including Explorer launches.
        interactive_terminal()
            && unsafe { GetConsoleProcessList(&mut process_id, 1) } == 1
            && process_id == std::process::id()
    }
    #[cfg(not(windows))]
    false
}

fn interactive_terminal() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

pub fn run() -> Result<()> {
    ensure!(
        interactive_terminal(),
        "--interactive requires an interactive terminal"
    );
    println!("Kindle Stripper interactive export\n");
    let books = match super::downloaded_books() {
        Ok(books) => {
            super::list_downloads(&books, true);
            books
        }
        Err(error) => {
            eprintln!("Could not list Kindle downloads: {error:#}");
            eprintln!("You can still enter a book folder directly.");
            Vec::new()
        }
    };
    let args = export_arguments(&mut io::stdin().lock(), &mut io::stdout().lock(), &books)?;
    if let Some(args) = args {
        super::run(args, true)?;
    }
    Ok(())
}

pub fn pause_on_exit() {
    let _ = prompt(
        &mut io::stdin().lock(),
        &mut io::stdout().lock(),
        "\nPress Enter to close...",
    );
}

fn export_arguments(
    input: &mut impl BufRead,
    output: &mut impl Write,
    books: &[(String, PathBuf)],
) -> io::Result<Option<Vec<OsString>>> {
    let (selection, batch) = loop {
        let Some(selection) = prompt(
            input,
            output,
            "\nEnter book numbers or ASINs (comma-separated), a book folder, or 'all' (Enter to quit): ",
        )?
        else {
            return Ok(None);
        };
        if selection.is_empty() {
            return Ok(None);
        }
        match resolve_book_selection(unquote(&selection), books) {
            Ok(selection) => break selection,
            Err(error) => writeln!(output, "{error:#}")?,
        }
    };
    let mut args = vec![selection.into()];

    let Some(convert) = choose_second(
        input,
        output,
        "Format [1] KFX archive (default), [2] EPUB/PDF: ",
    )?
    else {
        return Ok(None);
    };
    if convert {
        args.push("--convert".into());
        let Some(basic) =
            choose_second(input, output, "EPUB engine [1] Boko (default), [2] Basic: ")?
        else {
            return Ok(None);
        };
        if basic {
            args.extend(["--epub-engine".into(), "basic".into()]);
        }
    }

    let Some(directory) = prompt(
        input,
        output,
        "Output folder (created if missing, Enter for current directory): ",
    )?
    else {
        return Ok(None);
    };
    if !directory.is_empty() {
        args.extend(["--output-folder".into(), unquote(&directory).into()]);
    } else if !batch {
        let Some(path) = prompt(
            input,
            output,
            "Exact output path (Enter for generated filename): ",
        )?
        else {
            return Ok(None);
        };
        if !path.is_empty() {
            args.extend(["--output".into(), unquote(&path).into()]);
        }
    }
    Ok(Some(args))
}

fn resolve_book_selection(selection: &str, books: &[(String, PathBuf)]) -> Result<(String, bool)> {
    ensure!(!selection.is_empty(), "Enter a book selection.");
    ensure!(
        !selection.starts_with('-'),
        "Enter a book selection or use a full folder path for names starting with '-'."
    );
    if selection.eq_ignore_ascii_case("all") {
        return Ok(("--all".into(), true));
    }
    if selection.contains(['\\', '/']) || Path::new(selection).is_dir() {
        return Ok((selection.into(), false));
    }
    if selection.contains(',') {
        let resolved = selection
            .split(',')
            .map(|value| resolve_book_number(value.trim(), books))
            .collect::<Result<Vec<_>>>()?
            .join(",");
        return Ok((super::parse_asin_list(&resolved)?.join(","), true));
    }
    Ok((resolve_book_number(selection, books)?.into(), false))
}

fn resolve_book_number<'a>(value: &'a str, books: &'a [(String, PathBuf)]) -> Result<&'a str> {
    if value.is_empty() || super::is_asin(value) || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Ok(value);
    }
    let position = value
        .parse::<usize>()
        .ok()
        .and_then(|number| number.checked_sub(1))
        .and_then(|index| books.get(index))
        .with_context(|| format!("Book number {value} is outside the displayed list."))?;
    Ok(&position.0)
}

fn choose_second(
    input: &mut impl BufRead,
    output: &mut impl Write,
    label: &str,
) -> io::Result<Option<bool>> {
    while let Some(choice) = prompt(input, output, label)? {
        match choice.as_str() {
            "" | "1" => return Ok(Some(false)),
            "2" => return Ok(Some(true)),
            _ => writeln!(output, "Enter 1 or 2.")?,
        }
    }
    Ok(None)
}

fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value)
}

fn prompt(
    input: &mut impl BufRead,
    output: &mut impl Write,
    label: &str,
) -> io::Result<Option<String>> {
    write!(output, "{label}")?;
    output.flush()?;
    let mut line = String::new();
    if input.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    Ok(Some(line.trim().to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interactive_export_accepts_batch_conversion_and_quoted_paths() {
        let mut input = &b"all\n2\n2\n\"C:\\Kindle Exports\"\n"[..];
        let args = export_arguments(&mut input, &mut Vec::new(), &[]).unwrap();
        assert_eq!(
            args.unwrap(),
            [
                "--all",
                "--convert",
                "--epub-engine",
                "basic",
                "--output-folder",
                r"C:\Kindle Exports"
            ]
        );
    }

    #[test]
    fn interactive_export_accepts_single_book_and_exact_path() {
        let mut input = &b"B008E8XV56\n\n\nMy backup.kfx-zip\n"[..];
        let args = export_arguments(&mut input, &mut Vec::new(), &[]).unwrap();
        assert_eq!(
            args.unwrap(),
            ["B008E8XV56", "--output", "My backup.kfx-zip"]
        );
    }

    #[test]
    fn interactive_export_cancels_on_blank_selection_or_end_of_input() {
        for mut input in [&b"\n"[..], &b"B008E8XV56\n2\n"[..]] {
            assert!(
                export_arguments(&mut input, &mut Vec::new(), &[])
                    .unwrap()
                    .is_none()
            );
        }
    }

    #[test]
    fn interactive_selection_uses_the_displayed_order_and_accepts_asins() {
        let books = [
            ("B008E8XV56".into(), PathBuf::new()),
            ("B00XLII45G".into(), PathBuf::new()),
        ];
        for (selection, expected, batch) in [
            ("2", "B00XLII45G", false),
            ("2,1", "B00XLII45G,B008E8XV56", true),
            ("1, b00xlii45g", "B008E8XV56,B00XLII45G", true),
            ("B00XLII45G,B008E8XV56", "B00XLII45G,B008E8XV56", true),
            ("1234567890", "1234567890", false),
            ("ALL", "--all", true),
        ] {
            assert_eq!(
                resolve_book_selection(selection, &books).unwrap(),
                (expected.into(), batch)
            );
        }
        for selection in ["0", "3", "1,B008E8XV56", "1,", "all,1"] {
            assert!(
                resolve_book_selection(selection, &books).is_err(),
                "{selection}"
            );
        }
    }
}
