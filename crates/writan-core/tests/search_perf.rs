//! Search latency budget: warm queries against a 10 000-note index must stay
//! under 30 ms on average (PROGRESS.md perf budgets).
//!
//! Ignored by default — building and querying a 10k-note index is far too slow
//! in the debug profile. CI runs it explicitly in release mode:
//!
//! ```sh
//! cargo test -p writan-core --release --test search_perf -- --ignored --nocapture
//! ```
//!
//! The index is in-memory (the measured quantity is the trigram scan + bm25
//! ranking, not disk IO; a warm app cache behaves the same way in practice).

use std::time::{Duration, Instant};

use uuid::Uuid;
use writan_core::{Index, IndexedNote, SearchQuery};

const NOTES: usize = 10_000;
const POOL: u64 = 1024;
const BUDGET: Duration = Duration::from_millis(30);
/// Timed runs per query (after one warm-up run).
const RUNS: u32 = 21;

/// Small deterministic LCG (Numerical Recipes constants); no rand dependency.
fn next(seed: &mut u64) -> u64 {
    *seed = seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *seed >> 33
}

/// ~100 words out of a 1024-word pool, so a single pool word shows up in
/// roughly 9 % of all notes — the shape of a real content-word query.
fn make_body(seed: &mut u64, note: usize) -> String {
    let mut body = String::with_capacity(900);
    for _ in 0..100 {
        body.push_str("tok");
        body.push_str(&format!("{:04}", next(seed) % POOL));
        body.push(' ');
    }
    if note % 500 == 0 {
        body.push_str("xylocarp ");
    }
    body
}

fn build_index() -> Index {
    let mut index = Index::open_in_memory().expect("open index");
    let mut seed = 0x0DDB_1A5E_ED0D_D00D_u64;
    for i in 0..NOTES {
        let body = make_body(&mut seed, i);
        let note = IndexedNote {
            id: Uuid::from_u128(i as u128 + 1),
            rel_path: format!("notes/note-{i:05}.md"),
            title: format!("Note {i}"),
            tags: vec![format!("tag{}", i % 4)],
            pinned: false,
            created: None,
            updated: None,
            mtime_ns: i as i64,
            size: body.len() as i64,
            hash: String::new(),
        };
        index.upsert(&note, &body).expect("upsert");
    }
    index
}

/// Warms up once, then times `RUNS` runs; asserts a plausible hit count and
/// returns the average. Prints hits/avg/max for the CI log.
fn measure(index: &Index, what: &str, query: &SearchQuery, expect_min: usize) -> Duration {
    let hits = index.search(query).expect("search");
    assert!(
        hits.len() >= expect_min,
        "{what}: expected at least {expect_min} hits, got {}",
        hits.len()
    );

    let mut total = Duration::ZERO;
    let mut max = Duration::ZERO;
    for _ in 0..RUNS {
        let started = Instant::now();
        let _ = index.search(query).expect("search");
        let took = started.elapsed();
        total += took;
        max = max.max(took);
    }
    let avg = total / RUNS;
    println!(
        "{what:<24} hits={:<5} avg={:>7.2} ms  max={:>7.2} ms",
        hits.len(),
        avg.as_secs_f64() * 1e3,
        max.as_secs_f64() * 1e3
    );
    avg
}

#[test]
#[ignore = "perf budget; runs in release from CI (see the module docs)"]
fn search_stays_within_budget_on_10k_notes() {
    let build_started = Instant::now();
    let index = build_index();
    println!(
        "built index: {NOTES} notes in {:.1} s ({} rows)",
        build_started.elapsed().as_secs_f64(),
        index.count().expect("count"),
    );

    let mut results = Vec::new();
    results.push((
        "common word",
        measure(
            &index,
            "common word",
            &SearchQuery {
                text: "tok0421".into(),
                ..Default::default()
            },
            100,
        ),
    ));
    results.push((
        "two-word phrase",
        measure(
            &index,
            "two-word phrase",
            &SearchQuery {
                text: "tok0421 tok0055".into(),
                ..Default::default()
            },
            1,
        ),
    ));
    results.push((
        "tag + word",
        measure(
            &index,
            "tag + word",
            &SearchQuery {
                text: "tok0421".into(),
                tags: vec!["tag2".into()],
                ..Default::default()
            },
            10,
        ),
    ));
    results.push((
        "rare word",
        measure(
            &index,
            "rare word",
            &SearchQuery {
                text: "xylocarp".into(),
                ..Default::default()
            },
            20,
        ),
    ));
    results.push((
        "no match",
        measure(
            &index,
            "no match",
            &SearchQuery {
                text: "zzzqx".into(),
                ..Default::default()
            },
            0,
        ),
    ));

    if cfg!(debug_assertions) {
        println!("debug build — budget assertions skipped");
        return;
    }
    for (what, avg) in results {
        assert!(
            avg <= BUDGET,
            "{what}: average {:.2} ms is over the {} ms budget",
            avg.as_secs_f64() * 1e3,
            BUDGET.as_millis()
        );
    }
}
