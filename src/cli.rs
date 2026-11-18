//! Käsurida: valikud, abi (`--help`) ja fail või kaust.
//!
//! Tee loetakse andestavalt, sest käsureal läheb see kergesti katki:
//! - tühikuga tee ilma jutumärkideta tuleb mitme argumendina — need
//!   pannakse tühikuga kokku;
//! - `"D:\Pildid\"`: Windows teeb lõpu `\"`-st jutumärgi (`D:\Pildid"`) —
//!   lõpujutumärk ära;
//! - suhteline tee käivituskausta järgi.
//!
//! Teed, mida pole, ei vaigita: aken näitab põhjust.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub const USAGE: &str = "\
iview — fast picture viewer

Usage:
  iview [options] [file or folder]

  File: opens that picture; the arrow keys go through the folder's other pictures.
  Folder: opens the folder's first picture.
  A path with spaces works with or without quotes:
    iview \"D:\\My pictures\\IMG_0001.jpg\"

Options:
  -h, --help          this help
  -V, --version       version
  -f, --fullscreen    open in full screen (Esc closes)
  -n, --new-window    a new window, not the viewer that is already running
      --config FILE   settings file (default %APPDATA%\\iview\\config.toml)
      --timing        timing: startup.log and timing.log next to the settings
  --                  only a path follows (e.g. a file name starting with \"-\")

Developer:
      --export-iconset DIR   write the macOS icon set (tools/macos-app.sh)

Environment: IVIEW_CONFIG, IVIEW_TIMING (same as --config, --timing).
Keys: P opens the settings; the key row is under the picture.";

#[derive(Debug, Default, PartialEq)]
pub struct Cli {
    pub help: bool,
    pub version: bool,
    pub fullscreen: bool,
    pub new_window: bool,
    pub config: Option<PathBuf>,
    pub timing: bool,
    /// Arendajale (`tools/macos-app.sh`): Maci ikoonikomplekt sellesse kausta.
    pub export_iconset: Option<PathBuf>,
    /// Tee nagu käsureal (veel kontrollimata).
    pub path: Option<String>,
}

/// Valikud ja tee argumentidest (ilma programmi nimeta). `Err`: tundmatu
/// valik või puuduv väärtus.
pub fn parse(args: &[OsString]) -> Result<Cli, String> {
    let mut cli = Cli::default();
    let mut words: Vec<String> = Vec::new();
    let mut only_paths = false;
    let mut it = args.iter().map(|a| a.to_string_lossy().into_owned());
    while let Some(arg) = it.next() {
        if only_paths || !(arg.starts_with('-') || arg == "/?") || arg == "-" {
            words.push(arg);
            continue;
        }
        match arg.as_str() {
            "--" => only_paths = true,
            "-h" | "--help" | "/?" | "-?" => cli.help = true,
            "-V" | "--version" => cli.version = true,
            "-f" | "--fullscreen" => cli.fullscreen = true,
            "-n" | "--new-window" => cli.new_window = true,
            "--timing" => cli.timing = true,
            "--config" => cli.config = Some(PathBuf::from(it.next().ok_or("--config needs a file name")?)),
            "--export-iconset" => cli.export_iconset = Some(PathBuf::from(it.next().ok_or("--export-iconset needs a folder")?)),
            // macOS (vanem) lisab Finderist käivitades protsessi numbri.
            a if a.starts_with("-psn_") => {}
            a if a.starts_with("--config=") => cli.config = Some(PathBuf::from(&a["--config=".len()..])),
            a => return Err(format!("Unknown option: {a}")),
        }
    }
    if !words.is_empty() {
        cli.path = Some(words.join(" "));
    }
    Ok(cli)
}

/// Käsurea tee päris teeks: jutumärgid ümbert ja lõpust ära, suhteline
/// `cwd` järgi. `Err`: sellist faili ega kausta pole (põhjus aknasse).
pub fn resolve(raw: &str, cwd: &Path) -> Result<PathBuf, String> {
    let text = raw.trim().trim_matches('"').trim();
    if text.is_empty() {
        return Err("The path is empty".to_string());
    }
    let mut path = PathBuf::from(text);
    if path.is_relative() {
        path = cwd.join(path);
    }
    if path.exists() {
        return Ok(std::path::absolute(&path).unwrap_or(path));
    }
    Err(format!("No such file or folder: {text}"))
}

/// Tekst konsooli, kust programm käivitati. Windowsi väljalaskel pole oma
/// konsooli (aknaprogramm) — liitume vanema omaga; kui seda pole
/// (käivitati Explorerist), teateaken.
#[cfg(all(windows, not(debug_assertions)))]
pub fn print(text: &str, error: bool) {
    use std::io::Write;
    use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    // SAFETY: lihtne süsteemikutse; viga tähendab, et vanemal konsooli pole.
    if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) }.is_ok()
        && let Ok(mut con) = std::fs::OpenOptions::new().write(true).open("CONOUT$")
    {
        // Käsuviip on juba ees (aknaprogrammi ei oodata): tühi rida enne.
        let _ = write!(con, "\r\n{}\r\n", text.replace('\n', "\r\n"));
        return;
    }
    use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MessageBoxW};
    use windows::core::HSTRING;
    let icon = if error { MB_ICONERROR } else { MB_ICONINFORMATION };
    // SAFETY: nulliga lõppevad stringid elavad kutse lõpuni.
    unsafe { MessageBoxW(None, &HSTRING::from(text), &HSTRING::from("iview"), MB_OK | icon) };
}

#[cfg(not(all(windows, not(debug_assertions))))]
pub fn print(text: &str, error: bool) {
    if error {
        eprintln!("{text}");
    } else {
        println!("{text}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn options_and_path() {
        let c = parse(&args(&["-f", "--new-window", "D:\\Pildid\\a.jpg"])).unwrap();
        assert!(c.fullscreen && c.new_window && !c.help);
        assert_eq!(c.path.as_deref(), Some("D:\\Pildid\\a.jpg"));
        assert!(parse(&args(&["/?"])).unwrap().help);
        assert!(parse(&args(&["--help"])).unwrap().help);
        assert_eq!(parse(&args(&["--config", "x.toml"])).unwrap().config, Some(PathBuf::from("x.toml")));
        assert_eq!(parse(&args(&["--config=y.toml"])).unwrap().config, Some(PathBuf::from("y.toml")));
        assert!(parse(&args(&["--config"])).is_err());
        assert_eq!(parse(&args(&["--export-iconset", "x.iconset"])).unwrap().export_iconset, Some(PathBuf::from("x.iconset")));
        assert_eq!(parse(&args(&["--bogus"])), Err("Unknown option: --bogus".into()));
        assert_eq!(parse(&args(&["-psn_0_1234"])).unwrap(), Cli::default());
        assert_eq!(parse(&args(&[])).unwrap(), Cli::default());
    }

    /// Tühikuga tee ilma jutumärkideta tuleb mitme sõnana — kokku; `--`
    /// järel on ka „-“-ga algav nimi tee.
    #[test]
    fn split_path_is_joined() {
        let c = parse(&args(&["D:\\Minu", "pildid\\a.jpg"])).unwrap();
        assert_eq!(c.path.as_deref(), Some("D:\\Minu pildid\\a.jpg"));
        let c = parse(&args(&["--", "-imelik.jpg"])).unwrap();
        assert_eq!(c.path.as_deref(), Some("-imelik.jpg"));
    }

    /// Lõpujutumärk (`"D:\x\"` → `D:\x"`) ja suhteline tee; puuduva puhul põhjus.
    #[test]
    fn resolve_is_forgiving_and_honest() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("minu pildid");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("a.jpg"), b"x").unwrap();
        let quoted = format!("{}\"", sub.display());
        assert_eq!(resolve(&quoted, Path::new("C:\\")).unwrap(), sub);
        assert_eq!(resolve("minu pildid\\a.jpg", dir.path()).unwrap(), sub.join("a.jpg"));
        assert_eq!(resolve("minu pildid/a.jpg", dir.path()).unwrap(), sub.join("a.jpg"));
        let err = resolve("pole.jpg", dir.path()).unwrap_err();
        assert!(err.contains("pole.jpg"));
    }
}
