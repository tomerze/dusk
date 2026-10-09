pub fn fleet_token() -> &'static str {
    include_str!(concat!(env!("OUT_DIR"), "/fleet_token"))
}
