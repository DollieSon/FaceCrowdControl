//! The same tests, run against every database backend, so features behave identically
//! whichever one DATABASE_URL picks.
//!
//! SQLite always runs (in memory). Postgres runs when FCC_TEST_POSTGRES_URL points at a
//! server whose user may create databases, e.g.
//!   docker run --rm -d -p 55432:5432 -e POSTGRES_PASSWORD=test postgres:17
//!   FCC_TEST_POSTGRES_URL=postgres://postgres:test@localhost:55432/postgres cargo test
//! Each test gets its own throwaway database.

use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};

use chrono::{Duration, Utc};
use fcc_server::store::{
    self, ActionStatus, NewChatMessage, NewGameAction, RecordOutcome, Store, StoreError, User,
};
use serde_json::json;
use sqlx::{Connection, PgConnection};

// ---- backends ------------------------------------------------------------

async fn sqlite() -> Arc<dyn Store> {
    store::connect("sqlite::memory:").await.unwrap()
}

struct PgDb {
    admin_url: String,
    name: String,
    store: Arc<dyn Store>,
}

impl PgDb {
    async fn drop_db(self) {
        drop(self.store);
        let mut admin = PgConnection::connect(&self.admin_url).await.unwrap();
        sqlx::query(&format!(
            "DROP DATABASE IF EXISTS {} WITH (FORCE)",
            self.name
        ))
        .execute(&mut admin)
        .await
        .unwrap();
    }
}

async fn postgres() -> Option<PgDb> {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let admin_url = std::env::var("FCC_TEST_POSTGRES_URL")
        .ok()
        .filter(|u| !u.is_empty())?;
    let name = format!(
        "fcc_test_{}_{}_{}",
        std::process::id(),
        Utc::now().timestamp_micros(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let mut admin = PgConnection::connect(&admin_url)
        .await
        .expect("FCC_TEST_POSTGRES_URL unreachable");
    sqlx::query(&format!("CREATE DATABASE {name}"))
        .execute(&mut admin)
        .await
        .unwrap();
    let (base, _) = admin_url.rsplit_once('/').expect("URL ends in /dbname");
    let store = store::connect(&format!("{base}/{name}")).await.unwrap();
    Some(PgDb {
        admin_url,
        name,
        store,
    })
}

/// Generates `sqlite::<test>` and `postgres::<test>` for every listed test function.
macro_rules! conformance {
    ($($name:ident),* $(,)?) => {
        mod sqlite {
            $( #[tokio::test] async fn $name() { super::$name(super::sqlite().await).await; } )*
        }
        mod postgres {
            $(
                #[tokio::test]
                async fn $name() {
                    let Some(db) = super::postgres().await else {
                        eprintln!("skipped: set FCC_TEST_POSTGRES_URL to run against Postgres");
                        return;
                    };
                    super::$name(db.store.clone()).await;
                    db.drop_db().await;
                }
            )*
        }
    };
}

conformance!(
    chat_creates_user_then_updates_it,
    resent_message_is_a_duplicate,
    transfer_moves_points,
    failed_transfer_changes_nothing,
    concurrent_transfers_never_overspend,
    awards_only_active_users,
    game_action_charges_and_refund_happens_once,
    game_action_needs_enough_points,
    done_action_is_not_refunded,
    open_and_stale_actions,
    export_import_round_trip,
    import_refuses_non_empty_database,
);

// ---- helpers -------------------------------------------------------------

fn chat(user: &str, name: &str, message_id: &str, message: &str) -> NewChatMessage {
    NewChatMessage {
        platform: "facebook".into(),
        platform_user_id: user.into(),
        display_name: name.into(),
        message_id: message_id.into(),
        message: message.into(),
        sent_at: Some(store::now()),
    }
}

async fn viewer(s: &Arc<dyn Store>, user: &str) -> User {
    let msg_id = format!("join-{user}-{}", Utc::now().timestamp_nanos_opt().unwrap());
    match s
        .record_chat(&chat(user, user, &msg_id, "hi"), store::now())
        .await
        .unwrap()
    {
        RecordOutcome::Recorded { user } => user,
        RecordOutcome::Duplicate => panic!("unexpected duplicate"),
    }
}

/// Gives every user seen in the last hour `amount` points.
async fn fund(s: &Arc<dyn Store>, amount: i64) {
    s.award_active_users(store::now() - Duration::hours(1), amount)
        .await
        .unwrap();
}

async fn points(s: &Arc<dyn Store>, id: i64) -> i64 {
    s.user(id).await.unwrap().unwrap().points
}

fn heal(user_id: i64, cost: i64) -> NewGameAction {
    NewGameAction {
        game: "sts2".into(),
        action: "heal".into(),
        params: json!({"amount": 5}),
        user_id,
        cost,
    }
}

// ---- tests ---------------------------------------------------------------

async fn chat_creates_user_then_updates_it(s: Arc<dyn Store>) {
    let t0 = store::now();
    let RecordOutcome::Recorded { user: first } = s
        .record_chat(&chat("100", "Maria", "m1", "hello"), t0)
        .await
        .unwrap()
    else {
        panic!("expected recorded")
    };
    assert_eq!(
        (first.points, first.display_name.as_str(), first.created_at),
        (0, "Maria", t0)
    );

    let t1 = t0 + Duration::seconds(5);
    let RecordOutcome::Recorded { user: second } = s
        .record_chat(&chat("100", "Maria S.", "m2", "again"), t1)
        .await
        .unwrap()
    else {
        panic!("expected recorded")
    };
    assert_eq!(second.id, first.id, "same platform id = same user");
    assert_eq!(
        second.display_name, "Maria S.",
        "display name follows the latest message"
    );
    assert_eq!(second.created_at, t0);
    assert_eq!(second.last_seen_at, t1);

    let other = viewer(&s, "200").await;
    assert_ne!(other.id, first.id);
}

async fn resent_message_is_a_duplicate(s: Arc<dyn Store>) {
    let m = chat("100", "Maria", "m1", "hello");
    assert!(matches!(
        s.record_chat(&m, store::now()).await.unwrap(),
        RecordOutcome::Recorded { .. }
    ));
    assert_eq!(
        s.record_chat(&m, store::now()).await.unwrap(),
        RecordOutcome::Duplicate
    );
    // Same message id on another platform is a different message.
    let mut yt = m.clone();
    yt.platform = "youtube".into();
    assert!(matches!(
        s.record_chat(&yt, store::now()).await.unwrap(),
        RecordOutcome::Recorded { .. }
    ));
    assert_eq!(s.export().await.unwrap().chat_messages.len(), 2);
}

async fn transfer_moves_points(s: Arc<dyn Store>) {
    let a = viewer(&s, "a").await;
    let b = viewer(&s, "b").await;
    fund(&s, 100).await;

    let t = s.transfer_points(a.id, b.id, 30).await.unwrap();
    assert_eq!((t.from.points, t.to.points, t.amount), (70, 130, 30));
    assert_eq!(points(&s, a.id).await, 70);
    assert_eq!(points(&s, b.id).await, 130);
}

async fn failed_transfer_changes_nothing(s: Arc<dyn Store>) {
    let a = viewer(&s, "a").await;
    let b = viewer(&s, "b").await;
    fund(&s, 10).await;

    assert!(matches!(
        s.transfer_points(a.id, b.id, 11).await,
        Err(StoreError::InsufficientPoints { have: 10, need: 11 })
    ));
    // v1 bug: giving to a missing user still took the sender's points.
    assert!(matches!(
        s.transfer_points(a.id, 9999, 5).await,
        Err(StoreError::UserNotFound(9999))
    ));
    assert!(matches!(
        s.transfer_points(9999, a.id, 5).await,
        Err(StoreError::UserNotFound(9999))
    ));
    assert_eq!(points(&s, a.id).await, 10);
    assert_eq!(points(&s, b.id).await, 10);
}

async fn concurrent_transfers_never_overspend(s: Arc<dyn Store>) {
    let a = viewer(&s, "a").await;
    let b = viewer(&s, "b").await;
    fund(&s, 100).await;

    // 25 transfers of 10 from a balance of 100, all at once (and some going the other way):
    // exactly 10 of a's can succeed.
    let mut tasks = Vec::new();
    for i in 0..25 {
        let s = s.clone();
        tasks.push(tokio::spawn(async move {
            (i, s.transfer_points(a.id, b.id, 10).await.is_ok())
        }));
    }
    let ok = futures_count(tasks).await;
    assert_eq!(ok, 10);
    assert_eq!(points(&s, a.id).await, 0);
    assert_eq!(points(&s, b.id).await, 200);
}

async fn futures_count(tasks: Vec<tokio::task::JoinHandle<(i32, bool)>>) -> usize {
    let mut ok = 0;
    for t in tasks {
        if t.await.unwrap().1 {
            ok += 1;
        }
    }
    ok
}

async fn awards_only_active_users(s: Arc<dyn Store>) {
    let old = store::now() - Duration::hours(2);
    let RecordOutcome::Recorded { user: idle } = s
        .record_chat(&chat("idle", "Idle", "m1", "zzz"), old)
        .await
        .unwrap()
    else {
        panic!()
    };
    let active = viewer(&s, "active").await;

    let n = s
        .award_active_users(store::now() - Duration::minutes(30), 1)
        .await
        .unwrap();
    assert_eq!(n, 1);
    assert_eq!(points(&s, active.id).await, 1);
    assert_eq!(points(&s, idle.id).await, 0);
}

async fn game_action_charges_and_refund_happens_once(s: Arc<dyn Store>) {
    let v = viewer(&s, "v").await;
    fund(&s, 100).await;

    let a = s
        .create_game_action(&heal(v.id, 30), store::now())
        .await
        .unwrap();
    assert_eq!(
        (a.status, a.cost, a.params.clone()),
        (ActionStatus::Pending, 30, json!({"amount": 5}))
    );
    assert_eq!(points(&s, v.id).await, 70);

    assert!(s.mark_action_delivered(a.id, store::now()).await.unwrap());
    assert!(
        !s.mark_action_delivered(a.id, store::now()).await.unwrap(),
        "only pending -> delivered"
    );

    let failed = s
        .finish_action(
            a.id,
            ActionStatus::Rejected,
            Some("not in combat"),
            store::now(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (failed.status, failed.error.as_deref()),
        (ActionStatus::Rejected, Some("not in combat"))
    );
    assert_eq!(points(&s, v.id).await, 100, "refunded");

    // A late result or the expiry timer must not refund again.
    assert_eq!(
        s.finish_action(a.id, ActionStatus::Expired, None, store::now())
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        s.finish_action(a.id, ActionStatus::Done, None, store::now())
            .await
            .unwrap(),
        None
    );
    assert_eq!(points(&s, v.id).await, 100);
    assert_eq!(
        s.game_action(a.id).await.unwrap().unwrap().status,
        ActionStatus::Rejected
    );
}

async fn game_action_needs_enough_points(s: Arc<dyn Store>) {
    let v = viewer(&s, "v").await;
    fund(&s, 10).await;
    assert!(matches!(
        s.create_game_action(&heal(v.id, 30), store::now()).await,
        Err(StoreError::InsufficientPoints { have: 10, need: 30 })
    ));
    assert!(matches!(
        s.create_game_action(&heal(9999, 1), store::now()).await,
        Err(StoreError::UserNotFound(9999))
    ));
    assert_eq!(points(&s, v.id).await, 10);
    assert!(s.open_actions("sts2").await.unwrap().is_empty());
}

async fn done_action_is_not_refunded(s: Arc<dyn Store>) {
    let v = viewer(&s, "v").await;
    fund(&s, 50).await;
    let a = s
        .create_game_action(&heal(v.id, 50), store::now())
        .await
        .unwrap();
    let done = s
        .finish_action(a.id, ActionStatus::Done, None, store::now())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(done.status, ActionStatus::Done);
    assert_eq!(points(&s, v.id).await, 0);
}

async fn open_and_stale_actions(s: Arc<dyn Store>) {
    let v = viewer(&s, "v").await;
    fund(&s, 100).await;
    let t0 = store::now() - Duration::minutes(10);
    let old = s.create_game_action(&heal(v.id, 1), t0).await.unwrap();
    let delivered = s.create_game_action(&heal(v.id, 1), t0).await.unwrap();
    s.mark_action_delivered(delivered.id, t0).await.unwrap();
    let finished = s.create_game_action(&heal(v.id, 1), t0).await.unwrap();
    s.finish_action(finished.id, ActionStatus::Done, None, t0)
        .await
        .unwrap();
    let fresh = s
        .create_game_action(&heal(v.id, 1), store::now())
        .await
        .unwrap();
    let mut other_game = heal(v.id, 1);
    other_game.game = "isaac".into();
    s.create_game_action(&other_game, store::now())
        .await
        .unwrap();

    let open: Vec<i64> = s
        .open_actions("sts2")
        .await
        .unwrap()
        .iter()
        .map(|a| a.id)
        .collect();
    assert_eq!(open, vec![old.id, delivered.id, fresh.id]);

    let stale = s
        .stale_action_ids(store::now() - Duration::minutes(1))
        .await
        .unwrap();
    assert_eq!(stale, vec![old.id, delivered.id]);
}

async fn export_import_round_trip(s: Arc<dyn Store>) {
    let a = viewer(&s, "a").await;
    let b = viewer(&s, "b").await;
    fund(&s, 100).await;
    s.transfer_points(a.id, b.id, 25).await.unwrap();
    let act = s
        .create_game_action(&heal(a.id, 10), store::now())
        .await
        .unwrap();
    s.finish_action(act.id, ActionStatus::Failed, Some("boom"), store::now())
        .await
        .unwrap();
    s.record_chat(
        &NewChatMessage {
            sent_at: None,
            ..chat("b", "B", "no-ts", "x")
        },
        store::now(),
    )
    .await
    .unwrap();

    let snap = s.export().await.unwrap();
    assert_eq!(
        (
            snap.users.len(),
            snap.chat_messages.len(),
            snap.game_actions.len()
        ),
        (2, 3, 1)
    );

    // Into a fresh database of the same kind *and* into SQLite (covers Postgres -> SQLite).
    let (same_kind, cleanup) = fresh_like(&s).await;
    for target in [same_kind, sqlite().await] {
        target.import(&snap).await.unwrap();
        assert_eq!(
            target.export().await.unwrap(),
            snap,
            "{} -> {}",
            s.backend(),
            target.backend()
        );
        // New rows must not collide with imported ids.
        let c = viewer(&target, "c").await;
        assert!(c.id > b.id.max(a.id));
    }
    if let Some(db) = cleanup {
        db.drop_db().await;
    }
}

async fn import_refuses_non_empty_database(s: Arc<dyn Store>) {
    viewer(&s, "a").await;
    let snap = s.export().await.unwrap();
    assert!(matches!(s.import(&snap).await, Err(StoreError::NotEmpty)));
}

/// An empty database of the same backend as `s` (plus the Postgres database to drop afterwards).
async fn fresh_like(s: &Arc<dyn Store>) -> (Arc<dyn Store>, Option<PgDb>) {
    match s.backend() {
        "postgres" => {
            let db = postgres().await.unwrap();
            (db.store.clone(), Some(db))
        }
        _ => (sqlite().await, None),
    }
}
