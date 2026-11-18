//! Tekstimallid (`{name}`, `{taken:%d.%m.%Y}` …): Ctrl+S nimi ja
//! täisekraani tekst ja kuupäevakaust.

use std::path::{Component, Path};

use chrono::{NaiveDate, NaiveDateTime};

/// Malli tükid: tekst ja `{muutuja}` või `{muutuja:lisa}`.
#[derive(Debug, PartialEq)]
enum Piece {
    Text(String),
    Var { name: String, arg: Option<String> },
}

fn pieces(template: &str) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        let Some(len) = rest[start..].find('}') else { break };
        text.push_str(&rest[..start]);
        let inner = &rest[start + 1..start + len];
        let (name, arg) = match inner.split_once(':') {
            Some((n, a)) => (n.trim().to_ascii_lowercase(), Some(a.to_owned())),
            None => (inner.trim().to_ascii_lowercase(), None),
        };
        if name.is_empty() {
            text.push_str(&rest[start..start + len + 1]);
        } else {
            if !text.is_empty() {
                out.push(Piece::Text(std::mem::take(&mut text)));
            }
            out.push(Piece::Var { name, arg });
        }
        rest = &rest[start + len + 1..];
    }
    text.push_str(rest);
    if !text.is_empty() {
        out.push(Piece::Text(text));
    }
    out
}

/// Kas tekst on ainult eraldaja (tühikud ja kirjavahemärgid)?
fn is_separator(text: &str) -> bool {
    text.chars().all(|c| c.is_whitespace() || "·•|/\\-–—,;:()[]".contains(c))
}

/// Mall tekstiks: `lookup(nimi, lisa)` annab muutuja väärtuse. Muutuja,
/// millel väärtust pole, jääb koos oma eesoleva eraldajaga (või sildiga,
/// nt „ISO ") vahele; esimese väljundisse jõudva muutuja ees eraldajat ei
/// ole; mall, kus ükski muutuja ei anna midagi, on tühi.
pub fn render(template: &str, lookup: &mut dyn FnMut(&str, Option<&str>) -> Option<String>) -> String {
    let pieces = pieces(template);
    if !pieces.iter().any(|p| matches!(p, Piece::Var { .. })) {
        return template.to_owned();
    }
    let mut out = String::new();
    let mut lead = String::new();
    let mut emitted = false;
    for piece in pieces {
        match piece {
            Piece::Text(t) => lead.push_str(&t),
            Piece::Var { name, arg } => {
                let lead_text = std::mem::take(&mut lead);
                let Some(value) = lookup(&name, arg.as_deref()).filter(|v| !v.trim().is_empty()) else { continue };
                if emitted {
                    out.push_str(&lead_text);
                } else if !is_separator(&lead_text) {
                    // Silt enne esimest muutujat („Foto: ") jääb, eraldaja mitte.
                    out.push_str(lead_text.trim_start_matches(|c: char| c.is_whitespace() || "·•|/-–—,;:".contains(c)));
                }
                out.push_str(&value);
                emitted = true;
            }
        }
    }
    // Lõputekst ainult siis, kui viimane muutuja tuli; eraldaja lõpust maha.
    if emitted && !lead.is_empty() && !is_separator(&lead) {
        out.push_str(&lead);
    }
    out.trim().to_owned()
}

/// `{name:0-5}` → nime märgid 0 kuni 5 (lõpp kaasa arvamata).
pub fn slice_arg(value: &str, arg: Option<&str>) -> String {
    let Some(arg) = arg else { return value.to_owned() };
    let Some((a, b)) = arg.split_once('-') else { return value.to_owned() };
    let (Ok(a), Ok(b)) = (a.trim().parse::<usize>(), b.trim().parse::<usize>()) else { return value.to_owned() };
    value.chars().skip(a).take(b.saturating_sub(a)).collect()
}

/// Aeg mallis: `{taken:%d.%m.%Y}`; ilma lisata „2023-08-26 14:12".
pub fn time_arg(t: NaiveDateTime, arg: Option<&str>) -> String {
    use std::fmt::Write;
    let fmt = arg.filter(|a| !a.trim().is_empty()).unwrap_or("%Y-%m-%d %H:%M");
    // Vigane vorming (`%Q`) ei tohi paanikat tekitada: siis vaikimisi kuju.
    let mut out = String::new();
    if write!(out, "{}", t.format(fmt)).is_err() {
        return t.format("%Y-%m-%d %H:%M").to_string();
    }
    out
}

/// EXIF-i aeg („20230826-141233" nagu `exif::Meta` selle annab) ajaks.
pub fn parse_taken(text: &str) -> Option<NaiveDateTime> {
    let digits: String = text.chars().filter(char::is_ascii_digit).collect();
    if digits.len() < 14 {
        return None;
    }
    NaiveDateTime::parse_from_str(&digits[..14], "%Y%m%d%H%M%S").ok()
}

/// Failinimi Windowsi jaoks: keelatud märgid alakriipsuks, reserveeritud
/// nimed (CON, COM1 …) lisaga, lõpust punktid ja tühikud ära.
pub fn sanitize(segment: &str) -> String {
    let out: String = segment.chars().map(|c| if "<>:\"/\\|?*".contains(c) || (c as u32) < 0x20 { '_' } else { c }).collect();
    let mut out: String = out.trim_start().trim_end_matches(['.', ' ']).chars().take(200).collect();
    if out.is_empty() {
        return "_".into();
    }
    let stem = out.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL") || ((stem.starts_with("COM") || stem.starts_with("LPT")) && stem.len() == 4 && stem.as_bytes()[3].is_ascii_digit());
    if reserved {
        out.push('_');
    }
    out
}

/// Kas nimi sobib failinimeks (F2, Ctrl+S)? `Some(põhjus)`, kui ei sobi.
pub fn invalid_name(name: &str) -> Option<&'static str> {
    if name.trim().is_empty() {
        return Some("Enter a name");
    }
    if name.chars().any(|c| "<>:\"/\\|?*".contains(c) || (c as u32) < 0x20) {
        return Some("A name cannot contain < > : \" / \\ | ? *");
    }
    if name.ends_with(['.', ' ']) {
        return Some("A name cannot end with a dot or a space");
    }
    None
}

/// Kaustadest tulevad väärtused: kausta nimi, lähim kuupäevaga kaust ja
/// selle pealkiri („2022_07_14 (R6) Terminaator, Luke" → „Terminaator, Luke").
#[derive(Default, Debug, PartialEq)]
pub struct FolderVars {
    pub folder: String,
    pub date_folder: String,
    pub title: String,
}

pub fn folder_vars(source: &Path) -> FolderVars {
    let parent = source.parent().unwrap_or(Path::new(""));
    let mut v = FolderVars { folder: parent.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), ..FolderVars::default() };
    let names: Vec<String> = parent
        .components()
        .filter_map(|c| match c {
            Component::Normal(n) => Some(n.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    if let Some(at) = names.iter().rposition(|n| find_date(n).is_some())
        && let Some((start, len, _)) = find_date(&names[at])
    {
        v.title = folder_title(&names[at], start, len);
        v.date_folder = names[at].clone();
    }
    v
}

/// „2020.01.01", „2020 01 01" (ka segaeraldajad), „20200101" (ka kellaajaga
/// „20200101145100"), päev eespool „01.01.2020" / „1.1.2020". Peab olema päris
/// kuupäev (aasta 1900–2099). Tagastab alguse, pikkuse ja päeva.
fn find_date(name: &str) -> Option<(usize, usize, NaiveDate)> {
    let b = name.as_bytes();
    let digit = |i: usize| b.get(i).is_some_and(u8::is_ascii_digit);
    let sep = |i: usize| matches!(b.get(i), Some(b'_' | b'-' | b'.' | b' '));
    let run = |i: usize, max: usize| -> Option<(u32, usize)> {
        let n = (0..max).take_while(|&k| digit(i + k)).count();
        if n == 0 || digit(i + n) {
            return None;
        }
        Some((name[i..i + n].parse().ok()?, n))
    };
    let day = |y: u32, m: u32, d: u32| (1900..2100).contains(&y).then(|| NaiveDate::from_ymd_opt(y as i32, m, d)).flatten();
    let ymd = |i: usize| -> Option<(usize, NaiveDate)> {
        if let Some((y, 4)) = run(i, 4)
            && sep(i + 4)
            && let Some((m, ml)) = run(i + 5, 2)
            && sep(i + 5 + ml)
            && let Some((d, dl)) = run(i + 6 + ml, 2)
        {
            return Some((6 + ml + dl, day(y, m, d)?));
        }
        None
    };
    let compact = |i: usize| -> Option<(usize, NaiveDate)> {
        let n = (0..15).take_while(|&k| digit(i + k)).count();
        if n != 8 && n != 14 {
            return None;
        }
        let num = |a: usize, l: usize| name[i + a..i + a + l].parse::<u32>().ok();
        if n == 14 && !(num(8, 2)? < 24 && num(10, 2)? < 60 && num(12, 2)? < 60) {
            return None;
        }
        Some((8, day(num(0, 4)?, num(4, 2)?, num(6, 2)?)?))
    };
    let dmy = |i: usize| -> Option<(usize, NaiveDate)> {
        let (d, dl) = run(i, 2)?;
        if !sep(i + dl) {
            return None;
        }
        let (m, ml) = run(i + dl + 1, 2)?;
        if !sep(i + dl + 1 + ml) {
            return None;
        }
        let (y, 4) = run(i + dl + ml + 2, 4)? else { return None };
        Some((dl + ml + 6, day(y, m, d)?))
    };
    (0..b.len())
        .filter(|&i| digit(i) && (i == 0 || !b[i - 1].is_ascii_digit()))
        .find_map(|i| ymd(i).or_else(|| compact(i)).or_else(|| dmy(i)).map(|(len, d)| (i, len, d)))
}

/// Kuupäevakausta pealkiri: kuupäev ja kaamera märk („(R6)") ära.
fn folder_title(name: &str, start: usize, len: usize) -> String {
    let trim = |s: &str| s.trim_matches([' ', '_', '-', ',']).to_owned();
    let rest = format!("{} {}", trim(&name[..start]), trim(&name[start + len..]));
    let rest = rest.trim();
    let rest = match rest.strip_prefix('(').and_then(|r| r.split_once(')')) {
        Some((_, after)) => after,
        None => rest,
    };
    trim(rest)
}

/// Kuvasuhe „3:2", „16:9"; kui lihtsat ei ole, „1.50:1".
pub fn ratio_text(w: u32, h: u32) -> String {
    fn gcd(a: u32, b: u32) -> u32 {
        if b == 0 { a } else { gcd(b, a % b) }
    }
    if w == 0 || h == 0 {
        return String::new();
    }
    let g = gcd(w, h);
    let (a, b) = (w / g, h / g);
    if a <= 32 && b <= 32 { format!("{a}:{b}") } else { format!("{:.2}:1", f64::from(w) / f64::from(h)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(name: &str, arg: Option<&str>) -> Option<String> {
        match name {
            "name" => Some(slice_arg("IMG_4821", arg)),
            "folder" => Some("2023_08_26".into()),
            "index" => Some("12".into()),
            "total" => Some("240".into()),
            "camera" => Some("Canon EOS R6".into()),
            "iso" => None,
            "focal" => Some("35 mm".into()),
            "taken" => parse_taken("20230826-141233").map(|t| time_arg(t, arg)),
            _ => None,
        }
    }

    /// Puuduv muutuja kaob koos eraldajaga; silt enne puuduvat kaob ka;
    /// esimese ette eraldajat ei jää; aja vorming ja nimeosa töötavad.
    #[test]
    fn template_drops_missing_values_with_their_separators() {
        let r = |t: &str| render(t, &mut lookup);
        assert_eq!(r("{folder} › {name}  {index} / {total}"), "2023_08_26 › IMG_4821  12 / 240");
        assert_eq!(r("{camera} · {focal} · ISO {iso} · {taken:%d.%m.%Y}"), "Canon EOS R6 · 35 mm · 26.08.2023");
        assert_eq!(r("{iso} · {focal}"), "35 mm");
        assert_eq!(r("Foto: {name:0-3} ({iso})"), "Foto: IMG");
        assert_eq!(r("{lens} · {iso}"), "");
        assert_eq!(r("ainult tekst"), "ainult tekst");
        assert_eq!(render("{name}_{long_side}", &mut |n, a| if n == "long_side" { Some("2048".into()) } else { lookup(n, a) }), "IMG_4821_2048");
        assert_eq!(r("{taken}"), "2023-08-26 14:12");
    }

    /// Vigane ajavorming ei kuku, annab vaikimisi kuju.
    #[test]
    fn bad_time_format_falls_back() {
        let t = parse_taken("20230826-141233").unwrap();
        assert_eq!(time_arg(t, Some("%Q")), "2023-08-26 14:12");
    }

    #[test]
    fn date_folder_and_title() {
        let v = folder_vars(Path::new(r"K:\Digi\Digi_2022\2022_07_14 (R6) Terminaator, Luke\RAW\IMG_0001.CR3"));
        assert_eq!(v.folder, "RAW");
        assert_eq!(v.date_folder, "2022_07_14 (R6) Terminaator, Luke");
        assert_eq!(v.title, "Terminaator, Luke");
        assert_eq!(folder_vars(Path::new(r"C:\Pildid\a.jpg")).date_folder, "");
    }

    #[test]
    fn names_and_ratios() {
        assert_eq!(sanitize("a:b?.jpg. "), "a_b_.jpg");
        assert_eq!(sanitize("CON"), "CON_");
        assert!(invalid_name("a/b").is_some() && invalid_name("hea nimi").is_none() && invalid_name(" ").is_some());
        assert_eq!(ratio_text(6000, 4000), "3:2");
        assert_eq!(ratio_text(1920, 1080), "16:9");
        assert_eq!(ratio_text(1000, 333), "3.00:1");
    }
}
