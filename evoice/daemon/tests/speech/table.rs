use std::collections::BTreeMap;

use e_voice_daemon::speech::table::{CLIENT, VoiceTable};

fn table() -> VoiceTable {
    VoiceTable::new(BTreeMap::from([
        ("lens".to_owned(), "narrator".to_owned()),
        ("probe".to_owned(), "p".to_owned()),
    ]))
}

#[test]
fn test_the_header_name_is_lowercase_so_a_header_map_finds_it() {
    assert_eq!(CLIENT, "x-evoice-client");
}

#[test]
fn test_the_asked_voice_wins_then_the_callers_then_none() {
    let table = table();
    assert_eq!(
        table.voice(Some("ef_dora".to_owned()), Some("lens")).as_deref(),
        Some("ef_dora")
    );
    assert_eq!(table.voice(None, Some("lens")).as_deref(), Some("narrator"));
    assert_eq!(table.voice(None, Some("probe")).as_deref(), Some("p"));
    assert_eq!(table.voice(None, Some("stranger")), None);
    assert_eq!(table.voice(None, Some("LENS")), None);
    assert_eq!(table.voice(None, Some("")), None);
    assert_eq!(table.voice(None, None), None);
    assert_eq!(table.voice(Some("x".to_owned()), None).as_deref(), Some("x"));
}

#[test]
fn test_an_empty_table_never_gives_a_voice_and_a_clone_shares_the_answers() {
    let empty = VoiceTable::default();
    assert_eq!(empty.voice(None, Some("lens")), None);
    let copy = table().clone();
    assert_eq!(copy.voice(None, Some("lens")).as_deref(), Some("narrator"));
}
