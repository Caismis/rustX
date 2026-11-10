/* Read-only VM-step probe, linked to the exact bundled SQLite release archive.
 * This is evidence tooling, not a production change. No ANALYZE or schema edits.
 */
#include <sqlite3.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static void check(sqlite3 *db, int code) {
    if (code != SQLITE_OK) {
        fprintf(stderr, "%s\n", sqlite3_errmsg(db));
        exit(1);
    }
}
int main(int argc, char **argv) {
    if (argc != 2) return 2;
    sqlite3 *db = NULL;
    check(db, sqlite3_open_v2(argv[1], &db, SQLITE_OPEN_READONLY, NULL));
    sqlite3_stmt *head = NULL;
    check(db, sqlite3_prepare_v2(db, "SELECT max(position) FROM message_ledger", -1, &head, NULL));
    if (sqlite3_step(head) != SQLITE_ROW) return 3;
    sqlite3_int64 total = sqlite3_column_int64(head, 0);
    sqlite3_finalize(head);
    const char *queries[] = {
        "SELECT h.header_json FROM observation_headers h WHERE h.ledger_position>?1 AND h.ledger_position<=?2 AND h.reference_kind='message' ORDER BY h.ledger_position LIMIT ?3",
        "SELECT h.header_json FROM observation_headers h INDEXED BY observation_headers_ledger_idx WHERE h.ledger_position>?1 AND h.ledger_position<=?2 AND h.reference_kind='message' ORDER BY h.ledger_position LIMIT ?3"
    };
    printf("{\"sqlite\":\"%s\",\"ledger_rows\":%lld,\"queries\":[", sqlite3_libversion(), (long long)total);
    for (int i = 0; i < 2; ++i) {
        char explain[1024];
        snprintf(explain, sizeof(explain), "EXPLAIN QUERY PLAN %s", queries[i]);
        sqlite3_stmt *plan = NULL;
        check(db, sqlite3_prepare_v2(db, explain, -1, &plan, NULL));
        sqlite3_bind_int64(plan, 1, total - 8);
        sqlite3_bind_int64(plan, 2, total);
        sqlite3_bind_int(plan, 3, 8);
        printf("%s{\"forced_existing_index\":%s,\"plan\":[", i ? "," : "", i ? "true" : "false");
        int n = 0;
        while (sqlite3_step(plan) == SQLITE_ROW)
            printf("%s\"%s\"", n++ ? "," : "", sqlite3_column_text(plan, 3));
        sqlite3_finalize(plan);
        sqlite3_stmt *statement = NULL;
        check(db, sqlite3_prepare_v2(db, queries[i], -1, &statement, NULL));
        sqlite3_bind_int64(statement, 1, total - 8);
        sqlite3_bind_int64(statement, 2, total);
        sqlite3_bind_int(statement, 3, 8);
        int rows = 0, code;
        while ((code = sqlite3_step(statement)) == SQLITE_ROW) rows++;
        if (code != SQLITE_DONE) check(db, code);
        printf("],\"rows\":%d,\"vm_steps\":%d,\"sorts\":%d}", rows,
               sqlite3_stmt_status(statement, SQLITE_STMTSTATUS_VM_STEP, 0),
               sqlite3_stmt_status(statement, SQLITE_STMTSTATUS_SORT, 0));
        sqlite3_finalize(statement);
    }
    printf("]}\n");
    sqlite3_close(db);
    return 0;
}
