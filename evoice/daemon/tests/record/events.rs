use e_voice_daemon::schema::event::{DaemonEvent, RecordState};

#[tokio::test]
async fn test_every_recording_state_is_a_lowercase_tagged_event() {
    for (state, name) in [
        (RecordState::Started, "started"),
        (RecordState::Heard, "heard"),
        (RecordState::Finished, "finished"),
        (RecordState::Learned, "learned"),
        (RecordState::Failed, "failed"),
    ] {
        assert_eq!(
            serde_json::to_string(&DaemonEvent::Recording { state }).unwrap(),
            format!(r#"{{"type":"recording","state":"{name}"}}"#)
        );
    }
}
