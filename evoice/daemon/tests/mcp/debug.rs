use std::path::PathBuf;
use std::time::Duration;

use e_voice_core::schema::lang::Lang;

use crate::fake::{Device, FakeSpeech, FakeVoices, wired_with};

const SECRET: &str = "SECRET-c0ffee";

fn clean(what: &str, shown: &str) {
    assert!(!shown.contains(SECRET), "{what} prints the approver secret:\n{shown}");
    assert!(
        !shown.contains("c0ffee"),
        "{what} prints part of the approver secret:\n{shown}"
    );
}

#[tokio::test]
async fn test_no_debug_output_of_the_approval_desk_or_what_holds_it_prints_the_secret() {
    let w = wired_with(
        FakeSpeech::marked(),
        Device::new(0, 0),
        Duration::from_secs(5),
        FakeVoices::default(),
        std::collections::BTreeMap::new(),
    );
    let inbox = w
        .approvals
        .open(SECRET.to_owned(), Lang::Es, PathBuf::from("/work"))
        .expect("the desk is free");
    for (what, plain, pretty) in [
        ("Approvals", format!("{:?}", w.approvals), format!("{:#?}", w.approvals)),
        ("ApprovalInbox", format!("{inbox:?}"), format!("{inbox:#?}")),
        ("McpServer", format!("{:?}", w.state.mcp), format!("{:#?}", w.state.mcp)),
        ("AppState", format!("{:?}", w.state), format!("{:#?}", w.state)),
    ] {
        clean(what, &plain);
        clean(what, &pretty);
    }
    let shown = format!("{:?}", w.approvals);
    assert!(
        shown.contains("Approvals"),
        "the output must still say what it is: {shown}"
    );
}
