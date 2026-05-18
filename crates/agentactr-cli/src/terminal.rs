use std::io::IsTerminal;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ColorMode {
    Auto,
    Always,
    Never,
}

static COLOR_MODE: OnceLock<ColorMode> = OnceLock::new();

pub(crate) fn parse_global_color(args: &mut Vec<String>) -> Result<ColorMode, String> {
    if args.first().map(String::as_str) != Some("--color") {
        if args.iter().skip(1).any(|arg| arg == "--color") {
            return Err("--color must be a top-level global before the command".to_string());
        }
        return Ok(ColorMode::Auto);
    }
    let Some(value) = args.get(1).map(String::as_str) else {
        return Err("--color requires auto, always, or never".to_string());
    };
    let mode = match value {
        "auto" => ColorMode::Auto,
        "always" => ColorMode::Always,
        "never" => ColorMode::Never,
        other => {
            return Err(format!(
                "unsupported --color `{other}`; expected auto|always|never"
            ))
        }
    };
    args.drain(0..2);
    Ok(mode)
}

pub(crate) fn set_color_mode(mode: ColorMode) {
    let _ = COLOR_MODE.set(mode);
}

pub(crate) fn color_enabled(json_output: bool, local_override: Option<ColorMode>) -> bool {
    if json_output || std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    match local_override.unwrap_or_else(|| *COLOR_MODE.get().unwrap_or(&ColorMode::Auto)) {
        ColorMode::Always => true,
        ColorMode::Never => false,
        ColorMode::Auto => std::io::stdout().is_terminal(),
    }
}

pub(crate) fn paint(value: &str, code: &str, enabled: bool) -> String {
    if enabled {
        format!("\x1b[{code}m{value}\x1b[0m")
    } else {
        value.to_string()
    }
}

pub(crate) fn green(value: &str, enabled: bool) -> String {
    paint(value, "32", enabled)
}

pub(crate) fn red(value: &str, enabled: bool) -> String {
    paint(value, "31", enabled)
}

pub(crate) fn yellow(value: &str, enabled: bool) -> String {
    paint(value, "33", enabled)
}

pub(crate) fn cyan(value: &str, enabled: bool) -> String {
    paint(value, "36", enabled)
}

pub(crate) fn magenta(value: &str, enabled: bool) -> String {
    paint(value, "35", enabled)
}

pub(crate) fn dim(value: &str, enabled: bool) -> String {
    paint(value, "2", enabled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_color_must_be_before_command() {
        let mut args = vec![
            "tui".to_string(),
            "--color".to_string(),
            "never".to_string(),
        ];

        let err = parse_global_color(&mut args).unwrap_err();

        assert!(err.contains("top-level global"));
    }

    #[test]
    fn global_color_preparse_removes_color_args() {
        let mut args = vec![
            "--color".to_string(),
            "never".to_string(),
            "tui".to_string(),
            "latest".to_string(),
        ];

        let mode = parse_global_color(&mut args).unwrap();

        assert_eq!(mode, ColorMode::Never);
        assert_eq!(args, vec!["tui".to_string(), "latest".to_string()]);
    }
}
