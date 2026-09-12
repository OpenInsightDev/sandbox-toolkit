mod common;

use std::io::Cursor;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::http::{HeaderMap, Method};
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use quick_xml::Writer;

use sbx::auth::{AuthState, Credential};
use sbx::webdav::ls::{check_existing_exclusive, eval_if};
use sbx::webdav::{
    Depth, IfCondition, IfList, LockInfo, LockScope, PropEntry, PropRequest, clark_key,
    generate_lock_token, parse_clark, parse_depth, parse_destination, parse_if_header,
    parse_lock_token_header, parse_overwrite, parse_propfind_request, parse_proppatch_request,
    parse_timeout, xml as webdav_xml,
};
use sha_crypt::PasswordHasher;

fn bench_parse_if_header(c: &mut Criterion) {
    let mut group = c.benchmark_group("micro/parse_if_header");

    let mut h_simple = HeaderMap::new();
    h_simple.insert("if", "(<opaquelocktoken:t1>)".parse().unwrap());

    let mut h_complex = HeaderMap::new();
    h_complex.insert(
        "if",
        "</a> (<opaquelocktoken:t1>) (Not <DAV:no-lock>) </b> (<opaquelocktoken:t2>)"
            .parse()
            .unwrap(),
    );

    group.bench_function("simple_single_token", |b| {
        b.iter(|| parse_if_header(&h_simple));
    });

    group.bench_function("complex_multi_resource", |b| {
        b.iter(|| parse_if_header(&h_complex));
    });

    group.finish();
}

fn bench_parse_propfind_request(c: &mut Criterion) {
    let allprop = br#"<?xml version="1.0"?><D:propfind xmlns:D="DAV:"><D:allprop/></D:propfind>"#;
    let named = br#"<?xml version="1.0"?><D:propfind xmlns:D="DAV:"><D:prop><D:getcontentlength/><D:getetag/><D:resourcetype/><D:getlastmodified/><D:creationdate/></D:prop></D:propfind>"#;

    let mut group = c.benchmark_group("micro/parse_propfind_request");
    group.bench_function("allprop", |b| {
        b.iter(|| parse_propfind_request(allprop).unwrap());
    });
    group.bench_function("named_5_props", |b| {
        b.iter(|| parse_propfind_request(named).unwrap());
    });
    group.finish();
}

fn bench_parse_headers(c: &mut Criterion) {
    let mut group = c.benchmark_group("micro/parse_headers");

    let mut h_lt = HeaderMap::new();
    h_lt.insert("lock-token", "<opaquelocktoken:abc123>".parse().unwrap());

    let mut h_to = HeaderMap::new();
    h_to.insert("timeout", "Second-3600".parse().unwrap());

    let mut h_d = HeaderMap::new();
    h_d.insert("depth", "1".parse().unwrap());

    let mut h_dest = HeaderMap::new();
    h_dest.insert(
        "destination",
        "http://localhost:8080/docs/file.txt".parse().unwrap(),
    );

    let mut h_ow = HeaderMap::new();
    h_ow.insert("overwrite", "F".parse().unwrap());

    group.bench_function("lock_token_header", |b| {
        b.iter(|| parse_lock_token_header(&h_lt));
    });
    group.bench_function("timeout", |b| b.iter(|| parse_timeout(&h_to)));
    group.bench_function("depth", |b| b.iter(|| parse_depth(&h_d)));
    group.bench_function("destination", |b| b.iter(|| parse_destination(&h_dest)));
    group.bench_function("overwrite", |b| b.iter(|| parse_overwrite(&h_ow)));

    group.finish();
}

fn bench_method_try_from(c: &mut Criterion) {
    let mut group = c.benchmark_group("micro/method_try_from");
    group.bench_function("GET", |b| {
        b.iter(|| sbx::webdav::Method::try_from(&Method::GET));
    });
    group.bench_function("PROPFIND", |b| {
        b.iter(|| sbx::webdav::Method::try_from(&Method::from_bytes(b"PROPFIND").unwrap()));
    });
    group.bench_function("unknown", |b| {
        b.iter(|| sbx::webdav::Method::try_from(&Method::from_bytes(b"X-CUSTOM").unwrap()));
    });
    group.finish();
}

fn bench_auth_validate(c: &mut Criterion) {
    let mut plaintext_config = AuthState::new();
    plaintext_config.add_user("admin", "secret");

    let hash = sha_crypt::ShaCrypt::default()
        .hash_password("mypassword".as_bytes())
        .unwrap()
        .to_string();
    let mut sha_config = AuthState::new();
    sha_config
        .users
        .insert("admin".to_string(), Credential::Sha512Crypt(hash));

    let mut group = c.benchmark_group("micro/auth_validate");
    group.bench_function("plaintext_valid", |b| {
        b.iter(|| plaintext_config.validate("admin", "secret"));
    });
    group.bench_function("plaintext_invalid", |b| {
        b.iter(|| plaintext_config.validate("admin", "wrong"));
    });
    group.bench_function("sha512_valid", |b| {
        b.iter(|| sha_config.validate("admin", "mypassword"));
    });
    group.bench_function("sha512_invalid", |b| {
        b.iter(|| sha_config.validate("admin", "wrong"));
    });
    group.bench_function("unknown_user", |b| {
        b.iter(|| plaintext_config.validate("nobody", "secret"));
    });
    group.finish();
}

fn bench_xml_build_multistatus(c: &mut Criterion) {
    let entry = PropEntry::new("/file.txt".into(), UNIX_EPOCH, None, 42, false);

    let entry_dir = PropEntry::new("/subdir/".into(), UNIX_EPOCH, None, 0, true);

    let mut group = c.benchmark_group("micro/build_multistatus");

    for count in [1u32, 10, 100, 1000] {
        let mut entries: Vec<PropEntry> = Vec::with_capacity(count as usize);
        for i in 0..count {
            let mut e = if i % 5 == 0 {
                entry_dir.clone()
            } else {
                entry.clone()
            };
            if count <= 100 {
                e.href = format!("/file_{i}.txt");
            } else {
                e.href = format!("/file_{i:05}.txt");
            }
            entries.push(e);
        }

        group.throughput(criterion::Throughput::Elements(count as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(count),
            &entries,
            |b, entries| {
                b.iter(|| webdav_xml::build_multistatus(entries, &PropRequest::AllProp));
            },
        );
    }

    group.finish();
}

fn bench_xml_write_activelock(c: &mut Criterion) {
    let lock = LockInfo::new(
        LockScope::Exclusive,
        "opaquelocktoken:abc123".into(),
        Some("user@example.com".into()),
        SystemTime::now(),
        Some(Duration::from_secs(3600)),
        Depth::Zero,
    );

    let mut group = c.benchmark_group("micro/write_activelock");
    group.bench_function("exclusive_depth0", |b| {
        b.iter(|| {
            let mut w = Writer::new(Cursor::new(Vec::new()));
            webdav_xml::write_activelock(&mut w, &lock);
            w.into_inner().into_inner()
        });
    });
    group.finish();
}

fn bench_generate_lock_token(c: &mut Criterion) {
    let mut group = c.benchmark_group("micro/generate_lock_token");
    group.bench_function("generate", |b| b.iter(generate_lock_token));
    group.finish();
}

fn bench_clark(c: &mut Criterion) {
    let mut group = c.benchmark_group("micro/clark");
    group.bench_function("clark_key", |b| {
        b.iter(|| clark_key("http://example.com/ns", "prop0"));
    });
    group.bench_function("parse_clark_with_ns", |b| {
        b.iter(|| parse_clark("{http://example.com/ns}prop0"));
    });
    group.bench_function("parse_clark_no_ns", |b| {
        b.iter(|| parse_clark("prop0"));
    });
    group.finish();
}

fn bench_proppatch_parse(c: &mut Criterion) {
    let body = br#"<?xml version="1.0"?><D:propertyupdate xmlns:D="DAV:"><D:set><D:prop><X:p>val</X:p></D:prop></D:set><D:remove><D:prop><X:q/></D:prop></D:remove></D:propertyupdate>"#;

    let mut group = c.benchmark_group("micro/proppatch_parse");
    group.bench_function("set_and_remove", |b| {
        b.iter(|| parse_proppatch_request(body).unwrap());
    });
    group.finish();
}

fn bench_eval_if(c: &mut Criterion) {
    let lock = LockInfo::new(
        LockScope::Exclusive,
        "opaquelocktoken:t1".into(),
        None,
        SystemTime::now(),
        None,
        Depth::Zero,
    );
    let infos = vec![lock];

    let simple_list = vec![IfList::new(
        None,
        vec![IfCondition::StateToken("opaquelocktoken:t1".into())],
    )];
    let no_lock_list = vec![IfList::new(
        None,
        vec![IfCondition::StateToken("DAV:no-lock".into())],
    )];
    let not_list = vec![IfList::new(
        None,
        vec![IfCondition::Not(Box::new(IfCondition::StateToken(
            "DAV:no-lock".into(),
        )))],
    )];
    let tagged_list = vec![IfList::new(
        Some("/a".into()),
        vec![IfCondition::StateToken("opaquelocktoken:t1".into())],
    )];

    let mut group = c.benchmark_group("micro/eval_if");
    group.bench_function("simple_match", |b| {
        b.iter(|| eval_if(&simple_list, &infos, "/a"));
    });
    group.bench_function("no_lock_locked", |b| {
        b.iter(|| eval_if(&no_lock_list, &infos, "/a"));
    });
    group.bench_function("not_no_lock", |b| {
        b.iter(|| eval_if(&not_list, &infos, "/a"));
    });
    group.bench_function("tag_mismatch", |b| {
        b.iter(|| eval_if(&tagged_list, &infos, "/b"));
    });
    group.finish();
}

fn bench_check_exclusive(c: &mut Criterion) {
    let exclusive = vec![LockInfo::new(
        LockScope::Exclusive,
        "opaquelocktoken:t1".into(),
        None,
        SystemTime::now(),
        None,
        Depth::Zero,
    )];
    let shared = vec![LockInfo::new(
        LockScope::Shared,
        "opaquelocktoken:ts1".into(),
        None,
        SystemTime::now(),
        None,
        Depth::Zero,
    )];
    let empty: Vec<LockInfo> = vec![];

    let matching_tokens: Vec<String> = vec!["opaquelocktoken:t1".into()];
    let wrong_tokens: Vec<String> = vec!["opaquelocktoken:wrong".into()];
    let no_tokens: Vec<String> = vec![];

    let mut group = c.benchmark_group("micro/check_existing_exclusive");
    group.bench_function("empty", |b| {
        b.iter(|| check_existing_exclusive(&empty, &no_tokens));
    });
    group.bench_function("exclusive_matching", |b| {
        b.iter(|| check_existing_exclusive(&exclusive, &matching_tokens));
    });
    group.bench_function("exclusive_wrong_token", |b| {
        b.iter(|| check_existing_exclusive(&exclusive, &wrong_tokens));
    });
    group.bench_function("shared_only", |b| {
        b.iter(|| check_existing_exclusive(&shared, &no_tokens));
    });
    group.finish();
}

criterion_group!(
    micro,
    bench_parse_if_header,
    bench_parse_propfind_request,
    bench_parse_headers,
    bench_method_try_from,
    bench_auth_validate,
    bench_xml_build_multistatus,
    bench_xml_write_activelock,
    bench_generate_lock_token,
    bench_clark,
    bench_proppatch_parse,
    bench_eval_if,
    bench_check_exclusive,
);

criterion_main!(micro);
