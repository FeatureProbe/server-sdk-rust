use criterion::{black_box, criterion_group, criterion_main, Criterion};
use feature_probe_server_sdk::{load_json, FPUser, FeatureProbe};
use serde_json::json;
use std::{fs, path::PathBuf};

fn bench_bool_toggle(pair: (&FeatureProbe, &FPUser)) {
    let fp = pair.0;
    let user = pair.1;

    let _d = fp.bool_detail("bool_toogle", user, false);
}

fn bench_json_toggle(pair: (&FeatureProbe, &FPUser)) {
    let fp = pair.0;
    let user = pair.1;

    let _d = fp.json_detail("multi_condition_toggle", user, json!(""));
}

fn criterion_benchmark(c: &mut Criterion) {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("resources/fixtures/repo.json");
    let json_str = fs::read_to_string(path).unwrap();
    let repo = load_json(&json_str).unwrap();
    let user_default = FPUser::new();
    let user_hit = FPUser::new().with("city", "1");
    let fp = FeatureProbe::new_with("secret key".to_string(), repo);

    c.bench_function("bench_bool_toggle_defualt", |b| {
        b.iter(|| bench_bool_toggle(black_box((&fp, &user_default))))
    });

    c.bench_function("bench_bool_toggle_hit", |b| {
        b.iter(|| bench_bool_toggle(black_box((&fp, &user_hit))))
    });

    c.bench_function("bench_json_toggle_default", |b| {
        b.iter(|| bench_json_toggle(black_box((&fp, &user_default))))
    });

    c.bench_function("bench_json_toggle_hit", |b| {
        b.iter(|| bench_json_toggle(black_box((&fp, &user_hit))))
    });
}

criterion_group!(benches, criterion_benchmark);

criterion_main!(benches);
