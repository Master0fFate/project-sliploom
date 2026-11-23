pub(super) fn parts<'a>(name: Option<&'a str>, is_pdf: bool, prefix: &[u8]) -> (&'a str, &'a str) {
    let inferred = if is_pdf {
        Some("pdf")
    } else {
        sniff_raster(prefix)
    };
    let default = (
        if is_pdf { "document" } else { "image" },
        inferred.unwrap_or("bin"),
    );
    let Some(name) = name.filter(|name| portable(name)) else {
        return default;
    };
    let Some((stem, extension)) = name.rsplit_once('.') else {
        return inferred.map_or(default, |extension| (name, extension));
    };
    let compatible = if is_pdf {
        extension.eq_ignore_ascii_case("pdf")
    } else {
        inferred.is_some_and(|kind| raster_extension(extension) == Some(kind))
    };
    if compatible {
        (stem, extension)
    } else {
        default
    }
}

fn portable(name: &str) -> bool {
    if name.is_empty()
        || name.starts_with(['.', ' '])
        || name.ends_with(['.', ' '])
        || name.chars().any(|ch| {
            ch.is_control()
                || matches!(
                    ch,
                    '/' | '\\' | ':' | '"' | '<' | '>' | '|' | '?' | '*'
                        | '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}'
                        | '\u{2066}'..='\u{206f}'
                )
        })
    {
        return false;
    }
    let first = name.split_once('.').map_or(name, |(first, _)| first);
    let device = first.trim_end_matches(' ').to_ascii_uppercase();
    !matches!(
        device.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$" | "CONIN$" | "CONOUT$"
    ) && !device
        .strip_prefix("COM")
        .or_else(|| device.strip_prefix("LPT"))
        .is_some_and(|suffix| {
            matches!(
                suffix,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
}

fn raster_extension(extension: &str) -> Option<&'static str> {
    Some(match extension.to_ascii_lowercase().as_str() {
        "png" | "apng" => "png",
        "jpg" | "jpeg" => "jpg",
        "gif" => "gif",
        "webp" => "webp",
        "bmp" => "bmp",
        "tif" | "tiff" => "tiff",
        "ico" => "ico",
        "avif" => "avif",
        "heic" | "heif" => "heic",
        _ => return None,
    })
}

fn sniff_raster(prefix: &[u8]) -> Option<&'static str> {
    Some(match prefix {
        [0x89, b'P', b'N', b'G', 13, 10, 26, 10, ..] => "png",
        [0xff, 0xd8, 0xff, ..] => "jpg",
        [b'G', b'I', b'F', b'8', b'7' | b'9', b'a', ..] => "gif",
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P'] => "webp",
        [b'B', b'M', ..] => "bmp",
        [b'I', b'I', 42 | 43, 0, ..] | [b'M', b'M', 0, 42 | 43, ..] => "tiff",
        [0, 0, 1, 0, ..] => "ico",
        [
            _,
            _,
            _,
            _,
            b'f',
            b't',
            b'y',
            b'p',
            b'a',
            b'v',
            b'i',
            b'f' | b's',
        ] => "avif",
        [
            _,
            _,
            _,
            _,
            b'f',
            b't',
            b'y',
            b'p',
            b'h',
            b'e',
            b'i',
            b'c' | b'x',
        ] => "heic",
        _ => return None,
    })
}
