use super::*;
use crate::runtime::identity::UuidV7Generator;
use std::collections::VecDeque;
use std::sync::{Barrier, Mutex};

#[derive(Debug)]
struct Sequence(Mutex<VecDeque<uuid::Uuid>>);
impl UuidV7Generator for Sequence {
    fn next_uuid(&self) -> uuid::Uuid {
        let mut values = self.0.lock().unwrap();
        if values.len() > 1 {
            values.pop_front().unwrap()
        } else {
            *values.front().unwrap()
        }
    }
}
fn uuid(value: u64) -> uuid::Uuid {
    uuid::Uuid::parse_str(&format!("01900000-0000-7000-8000-{value:012}")).unwrap()
}
fn identities(values: &[u64]) -> Arc<dyn UuidV7Generator> {
    Arc::new(Sequence(Mutex::new(
        values.iter().map(|value| uuid(*value)).collect(),
    )))
}
fn intent() -> SessionPersistentState {
    SessionPersistentState {
        cwd: PathBuf::from("/workspace"),
        model: None,
    }
}

#[test]
fn injected_collisions_retry_and_publication_order_does_not_follow_uuid_order() {
    let directory = tempfile::tempdir().unwrap();
    let controller =
        crate::runtime::local_storage::ProductController::acquire(directory.path()).unwrap();
    let mut catalog = SessionCatalog::empty(&controller)
        .unwrap()
        .with_identity_generator(identities(&[900, 901, 902, 900, 901, 902, 100, 101, 102]));
    let first = catalog.prepare_session(&intent(), &[]).unwrap();
    catalog
        .publish_session(&first, SessionNodeOrigin::New)
        .unwrap();
    let second = catalog.prepare_session(&intent(), &[]).unwrap();
    catalog
        .publish_session(&second, SessionNodeOrigin::New)
        .unwrap();
    assert_eq!(first.session_id, SessionId::from_uuid(uuid(900)).unwrap());
    assert_eq!(second.session_id, SessionId::from_uuid(uuid(100)).unwrap());
    assert!(
        second.session_id < first.session_id,
        "UUID spelling deliberately reverses publication order"
    );
    assert_eq!(
        catalog
            .list_page(None, 0, 10)
            .unwrap()
            .sessions
            .iter()
            .map(|row| &row.id)
            .collect::<Vec<_>>(),
        [&first.session_id, &second.session_id]
    );
    let retained = fs::read(&first.database_path).unwrap();
    let reconstructed = SessionCatalog::open_existing(directory.path())
        .unwrap()
        .unwrap()
        .with_identity_generator(identities(&[902]));
    assert!(
        reconstructed.prepare_session(&intent(), &[]).is_err(),
        "collision budget is finite across reconstruction"
    );
    assert_eq!(fs::read(&first.database_path).unwrap(), retained);
}

#[test]
fn same_conversation_uuid_in_different_sessions_has_exactly_one_reservation_winner() {
    let directory = tempfile::tempdir().unwrap();
    let controller =
        crate::runtime::local_storage::ProductController::acquire(directory.path()).unwrap();
    let _catalog = SessionCatalog::empty(&controller).unwrap();
    let conversation = ConversationId::from_uuid(uuid(1)).unwrap();
    let barrier = Barrier::new(2);
    let results = std::thread::scope(|scope| {
        let workers: Vec<_> = [10, 20]
            .into_iter()
            .map(|value| {
                let barrier = &barrier;
                let product = &*controller;
                let conversation = &conversation;
                scope.spawn(move || {
                    let session = SessionId::from_uuid(uuid(value)).unwrap();
                    let allocation = conversation_database_path(
                        &product.root().join("sessions"),
                        &session,
                        conversation,
                    )
                    .parent()
                    .unwrap()
                    .to_owned();
                    barrier.wait();
                    // Mirrors production: reserve the identity first, then
                    // materialize the allocation directory only for the winner.
                    let result = product.reserve_conversation(conversation).and_then(|_| {
                        SessionCatalog::create_conversation_allocation(product, &allocation)
                    });
                    (allocation.clone(), result)
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(
        results.iter().filter(|(_, result)| result.is_ok()).count(),
        1
    );
    let winner = results.iter().find(|(_, result)| result.is_ok()).unwrap();
    let loser = results.iter().find(|(_, result)| result.is_err()).unwrap();
    assert_eq!(
        loser.1.as_ref().unwrap_err().kind(),
        std::io::ErrorKind::AlreadyExists
    );
    assert!(winner.0.is_dir());
    assert!(!loser.0.exists());
    let marker = winner.0.join("retained-output");
    fs::write(&marker, "retained").unwrap();
    assert!(controller.reserve_conversation(&conversation).is_err());
    assert_eq!(fs::read_to_string(marker).unwrap(), "retained");
}

#[test]
fn first_session_collision_refuses_without_overwriting_an_unpublished_reservation() {
    for collide_session in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        // A supported modern root; the legacy boundary is exercised separately.
        let product = crate::runtime::local_storage::ProductRoot::create(directory.path()).unwrap();
        let session = SessionId::from_uuid(uuid(10)).unwrap();
        let conversation = ConversationId::from_uuid(uuid(12)).unwrap();
        let reserved_session = directory.path().join("sessions").join(session.as_str());
        let reserved_conversation = reserved_session
            .join("conversations")
            .join(conversation.as_str());
        let allocation = if collide_session {
            // A reserved Session directory exists without a catalog entry.
            fs::create_dir_all(&reserved_conversation).unwrap();
            fs::write(
                reserved_conversation.join("conversation.sqlite"),
                b"existing durable bytes",
            )
            .unwrap();
            [10, 11, 13]
        } else {
            // The Conversation identity is already consumed in the reservation
            // namespace, independently of any allocation directory.
            product.reserve_conversation(&conversation).unwrap();
            [20, 21, 12]
        };
        let result = SessionCatalog::create_unpublished_with_identities(
            directory.path(),
            &intent(),
            identities(&allocation),
        );
        assert!(
            result.is_err(),
            "the first allocation must refuse a reserved identity"
        );
        if collide_session {
            assert_eq!(
                fs::read(reserved_conversation.join("conversation.sqlite")).unwrap(),
                b"existing durable bytes"
            );
        } else {
            assert!(product.reserve_conversation(&conversation).is_err());
        }
        assert!(!directory.path().join("sessions/catalog.json").exists());
    }
}
