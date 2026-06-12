use super::*;

#[test]
fn preferred_hook_port_uses_cduo_port_before_port() {
    let port = preferred_hook_port_from(|name| match name {
        "CDUO_PORT" => Some("54444".to_string()),
        "PORT" => Some("55555".to_string()),
        _ => None,
    });
    assert_eq!(port, 54444);
}

#[test]
fn preferred_hook_port_falls_back_to_default() {
    assert_eq!(preferred_hook_port_from(|_| None), 53333);
    assert_eq!(
        preferred_hook_port_from(|name| (name == "CDUO_PORT").then(|| "bad".to_string())),
        53333
    );
}
