use super::*;

#[test]
fn the_init_event_tells_what_a_server_offers() {
    let line = r#"{"type":"system","subtype":"init","mcp_servers":[{"name":"asana","status":"connected"}],"tools":["Read","mcp__asana__get_task"]}"#;
    let found = discovery_from_init(line, "asana").unwrap();
    assert_eq!(found.status, "ok");
    assert_eq!(found.tools[0].name, "get_task");
    let line = line.replace("connected", "needs-auth");
    assert_eq!(
        discovery_from_init(&line, "asana").unwrap().status,
        "needs-auth"
    );
}
