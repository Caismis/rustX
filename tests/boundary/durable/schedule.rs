//! Actual file-backed admission/reopen and independent SQLite writer races.
use crate::durable::{CanonicalStore, InboxStore, ScheduleStore, SqliteConversationStore};
use crate::runtime::identity::{ConversationId, SessionId, SessionNodeId};
use crate::runtime::types::RuntimeClock;
use crate::scheduler::*;
use chrono::{DateTime, Duration, Utc};
use std::sync::Arc;
struct Clock(DateTime<Utc>);
impl RuntimeClock for Clock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}
fn binding() -> ScheduleBinding {
    ScheduleBinding {
        session_id: SessionId::new("ses_36524fd8-f674-7fc2-8125-06d01fee0e18"),
        node_id: SessionNodeId::new("node_36524fd8-f674-7fc2-8125-06d01fee0e18"),
        conversation_id: ConversationId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e18"),
    }
}
fn now() -> Clock {
    Clock(
        DateTime::parse_from_rfc3339("2026-10-23T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc),
    )
}
fn create(store: &SqliteConversationStore, timing: ScheduleTiming) -> ScheduleCommandReceipt {
    let ScheduleCommandResult::Applied { receipt } = store
        .schedule_command(
            &binding(),
            &ScheduleCommand {
                command_id: "one".into(),
                mutation: ScheduleMutation::Create {
                    title: "Reminder".into(),
                    text: "check result".into(),
                    timing,
                },
            },
            &now(),
            &crate::scheduler::ScheduleCommandAuthority::RuntimeControl,
        )
        .unwrap()
    else {
        panic!()
    };
    receipt
}
#[test]
fn committed_occurrence_and_inbound_survive_reopen_without_refiring() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("conversation.sqlite");
    let store = SqliteConversationStore::open(binding().conversation_id, &path).unwrap();
    let created = create(&store, ScheduleTiming::After { seconds: 60 });
    let clock = Clock(now().0 + Duration::seconds(120));
    let ScheduleAdmission::Admitted { inbound, .. } = store
        .admit_scheduled_target(&created.id, 1, &clock)
        .unwrap()
    else {
        panic!()
    };
    drop(store);
    let reopened = SqliteConversationStore::open(binding().conversation_id, &path).unwrap();
    assert!(matches!(
        reopened
            .admit_scheduled_target(&created.id, 1, &clock)
            .unwrap(),
        ScheduleAdmission::AlreadyAdvanced { .. }
    ));
    assert_eq!(reopened.load_pending().unwrap()[0].message, inbound.message);
    assert!(reopened.load_canonical().unwrap().is_empty());
    assert_eq!(reopened.schedule_summary().unwrap().pending.len(), 1);
}
#[test]
fn recurring_saved_targets_and_latest_only_catchup_survive_repeated_reopen() {
    for timing in [
        ScheduleTiming::Interval { every_seconds: 600 },
        ScheduleTiming::Cron {
            expression: "*/10 * * * *".into(),
            time_zone: "UTC".into(),
        },
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("conversation.sqlite");
        let store = SqliteConversationStore::open(binding().conversation_id, &path).unwrap();
        let created = create(&store, timing);
        drop(store);
        let clock = Clock(now().0 + Duration::minutes(37));
        let reopened = SqliteConversationStore::open(binding().conversation_id, &path).unwrap();
        let ScheduleAdmission::Admitted { snapshot, .. } = reopened
            .admit_scheduled_target(&created.id, 1, &clock)
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(
            snapshot.last_trigger.unwrap().trigger_id.occurrence_utc,
            now().0 + Duration::minutes(30)
        );
        assert_eq!(snapshot.next_due_utc, Some(now().0 + Duration::minutes(40)));
        drop(reopened);
        let reopened = SqliteConversationStore::open(binding().conversation_id, &path).unwrap();
        assert!(matches!(
            reopened
                .admit_scheduled_target(&created.id, 2, &clock)
                .unwrap(),
            ScheduleAdmission::NotDue
        ));
        assert_eq!(reopened.load_pending().unwrap().len(), 1);
    }
}
#[test]
fn independent_sqlite_writers_cannot_commit_the_same_occurrence() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("conversation.sqlite");
    let store = SqliteConversationStore::open(binding().conversation_id, &path).unwrap();
    let created = create(&store, ScheduleTiming::After { seconds: 60 });
    let other = SqliteConversationStore::open(binding().conversation_id, &path).unwrap();
    let gate = Arc::new(std::sync::Barrier::new(3));
    let handles: Vec<_> = [store, other]
        .into_iter()
        .map(|store| {
            let gate = gate.clone();
            let id = created.id.clone();
            std::thread::spawn(move || {
                gate.wait();
                store
                    .admit_scheduled_target(&id, 1, &Clock(now().0 + Duration::seconds(60)))
                    .unwrap()
            })
        })
        .collect();
    gate.wait();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, ScheduleAdmission::Admitted { .. }))
            .count(),
        1
    );
    assert_eq!(
        SqliteConversationStore::open_existing(binding().conversation_id, &path)
            .unwrap()
            .load_pending()
            .unwrap()
            .len(),
        1
    );
}
