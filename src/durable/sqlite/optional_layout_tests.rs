use super::{Connection, ConversationStoreError, optional_display_layout, storage};

fn rows(connection: &Connection) -> Vec<i64> {
    connection
        .prepare("SELECT value FROM facts ORDER BY value")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[test]
fn cached_layout_savepoints_preserve_nested_failure_and_outer_commit_after_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("layout.sqlite");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch("CREATE TABLE facts(value INTEGER)")
        .unwrap();
    let transaction = connection.unchecked_transaction().unwrap();
    transaction
        .execute("INSERT INTO facts VALUES(1)", [])
        .unwrap();
    assert_eq!(
        optional_display_layout(&transaction, || {
            transaction
                .execute("INSERT INTO facts VALUES(2)", [])
                .unwrap();
            let failed: Option<()> = optional_display_layout(&transaction, || {
                transaction
                    .execute("INSERT INTO facts VALUES(3)", [])
                    .unwrap();
                Err(storage("derived layout failed"))
            });
            assert!(failed.is_none());
            assert_eq!(rows(&transaction), [1, 2]);
            Ok(7)
        }),
        Some(7)
    );
    let failed: Option<()> = optional_display_layout(&transaction, || {
        transaction
            .execute("INSERT INTO facts VALUES(4)", [])
            .unwrap();
        assert!(
            optional_display_layout(&transaction, || {
                transaction
                    .execute("INSERT INTO facts VALUES(5)", [])
                    .unwrap();
                Ok(())
            })
            .is_some()
        );
        Err(storage("outer derived layout failed"))
    });
    assert!(failed.is_none());
    // A schema change expires the cached statements. Reuse must still execute
    // the savepoint against the current connection and transaction.
    transaction
        .execute_batch("CREATE INDEX facts_value ON facts(value)")
        .unwrap();
    assert!(
        optional_display_layout(&transaction, || {
            transaction
                .execute("INSERT INTO facts VALUES(6)", [])
                .unwrap();
            Ok(())
        })
        .is_some()
    );
    transaction.commit().unwrap();
    drop(connection);
    assert_eq!(rows(&Connection::open(path).unwrap()), [1, 2, 6]);
}

#[test]
fn failed_layout_rollback_never_releases_unreverted_changes() {
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization, TransactionOperation};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch("CREATE TABLE facts(value INTEGER); BEGIN")
        .unwrap();
    let releases = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&releases);
    connection
        .authorizer(Some(move |context: AuthContext<'_>| {
            if let AuthAction::Savepoint { operation, .. } = context.action {
                if operation == TransactionOperation::Rollback {
                    return Authorization::Deny;
                }
                if operation == TransactionOperation::Release {
                    observed.fetch_add(1, Ordering::Relaxed);
                }
            }
            Authorization::Allow
        }))
        .unwrap();
    let failed = optional_display_layout(&connection, || {
        connection
            .execute("INSERT INTO facts VALUES(1)", [])
            .unwrap();
        Err::<(), ConversationStoreError>(storage("derived layout failed"))
    });
    assert!(failed.is_none());
    assert_eq!(releases.load(Ordering::Relaxed), 0);
    connection
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>)
        .unwrap();
    connection.execute_batch("ROLLBACK").unwrap();
    assert!(rows(&connection).is_empty());
}
