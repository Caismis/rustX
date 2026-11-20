pub const MAX_DISPLAY_ICON_BYTES: usize = 128;

pub(crate) fn valid_iconify_id(icon: &str) -> bool {
    icon.len() <= MAX_DISPLAY_ICON_BYTES
        && icon
            .split_once(':')
            .is_some_and(|(prefix, name)| icon_part(prefix) && icon_part(name))
}

fn icon_part(part: &str) -> bool {
    !part.is_empty()
        && !part.starts_with('-')
        && !part.ends_with('-')
        && !part.contains("--")
        && part
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}
