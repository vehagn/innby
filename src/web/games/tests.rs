use axum::{
    body::Body,
    http::{Request, StatusCode, header},
    response::Response,
};
use serde_json::{Value, json};
use sqlx::PgPool;

use crate::{
    db::{games, rsvp},
    web::testing::{Site, body, en},
};

const GAMES: &[(&str, &str)] = &[
    ("INNBY_GAMES_URL", "https://games.example.com"),
    (
        "INNBY_GAMES",
        "quiz:time:Quick quiz; pinball:points:Pinball",
    ),
];

const SCORES: &str = "/api/games/quiz/scores?lang=en";

/// A call from a game on the games' origin, with this token.
fn call(method: &str, uri: &str, token: &str, score: Option<i64>) -> Request<Body> {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::ORIGIN, "https://games.example.com")
        .header("sec-fetch-site", "same-site")
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    match score {
        Some(score) => request
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({ "score": score }).to_string())),
        None => request.body(Body::empty()),
    }
    .unwrap()
}

async fn json(response: Response) -> Value {
    serde_json::from_str(&body(response).await).unwrap()
}

#[sqlx::test]
async fn a_game_keeps_a_guests_best_with_their_token(db: PgPool) {
    let site = Site::with(db, GAMES);
    let (kari, _) = site.invite("Kari Nordmann", 1).await;
    let (ola, _) = site.invite("Ola Nordmann", 1).await;
    let party = site.event().await;
    games::offer(&site.db, party.id, &["quiz".into()])
        .await
        .unwrap();
    let token = |rsvp: &rsvp::Rsvp, game: &str| {
        site.config
            .link_key
            .score_token(rsvp.id, rsvp.link_nonce, game)
    };
    let kari_quiz = token(&kari, "quiz");

    // Kari's 12.3 s, then a slower round that isn't kept.
    let response = site
        .send(call("POST", SCORES, &kari_quiz, Some(12_300)))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
        "https://games.example.com"
    );
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    site.send(call("POST", SCORES, &kari_quiz, Some(15_000)))
        .await;
    site.send(call("POST", SCORES, &token(&ola, "quiz"), Some(9_000)))
        .await;

    // Names as on the guest list: Ola hasn't answered, so "Ola N.".
    let table = json(site.send(call("GET", SCORES, &kari_quiz, None)).await).await;
    assert_eq!(
        table,
        json!({
            "game": "quiz",
            "kind": "time",
            "scores": [
                { "name": "Ola N.", "score": 9000, "shown": "9.00 s", "you": false },
                { "name": "Kari N.", "score": 12300, "shown": "12.30 s", "you": true },
            ],
            "yours": 12300,
        })
    );
    // Hidden from the guest list, a guest is "A guest" here too.
    rsvp::set_name_display(&site.db, ola.id, rsvp::NameDisplay::Hidden)
        .await
        .unwrap();
    let table = json(site.send(call("GET", SCORES, &kari_quiz, None)).await).await;
    assert_eq!(table["scores"][0]["name"], en().a_guest.as_str());
}

#[sqlx::test]
async fn only_a_valid_token_for_an_offered_game_counts(db: PgPool) {
    let site = Site::with(db, GAMES);
    let (kari, _) = site.invite("Kari", 1).await;
    let party = site.event().await;
    games::offer(&site.db, party.id, &["quiz".into()])
        .await
        .unwrap();
    let key = &site.config.link_key;
    let quiz = key.score_token(kari.id, kari.link_nonce, "quiz");
    let status = |request| async { site.send(request).await.status() };

    // No token, someone else's game's, a forged one, or one past a new link.
    for token in [
        "",
        "nonsense",
        &key.score_token(kari.id, kari.link_nonce, "pinball"),
    ] {
        let pinball = token.contains('.');
        let uri = if pinball {
            "/api/games/quiz/scores"
        } else {
            SCORES
        };
        assert_eq!(
            status(call("POST", uri, token, Some(1))).await,
            StatusCode::UNAUTHORIZED
        );
    }
    rsvp::revoke_link(&site.db, kari.id).await.unwrap();
    assert_eq!(
        status(call("GET", SCORES, &quiz, None)).await,
        StatusCode::UNAUTHORIZED
    );

    let kari = site.find(&kari).await.unwrap();
    let quiz = key.score_token(kari.id, kari.link_nonce, "quiz");
    let pinball = key.score_token(kari.id, kari.link_nonce, "pinball");
    // A game in the config the event doesn't offer, and one nowhere.
    let uri = "/api/games/pinball/scores";
    assert_eq!(
        status(call("POST", uri, &pinball, Some(1))).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        status(call("GET", uri, &pinball, None)).await,
        StatusCode::NOT_FOUND
    );
    let uri = "/api/games/chess/scores";
    assert_eq!(
        status(call("GET", uri, &quiz, None)).await,
        StatusCode::NOT_FOUND
    );
    // Scores out of range.
    for score in [-1, games::MAX_SCORE + 1] {
        let response = site.send(call("POST", SCORES, &quiz, Some(score))).await;
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{score}"
        );
    }
}

#[sqlx::test]
async fn only_the_games_origin_may_call(db: PgPool) {
    let site = Site::with(db, GAMES);
    let preflight = |origin: &str| {
        Request::builder()
            .method("OPTIONS")
            .uri(SCORES)
            .header(header::ORIGIN, origin)
            .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
            .header(
                header::ACCESS_CONTROL_REQUEST_HEADERS,
                "authorization, content-type",
            )
            .body(Body::empty())
            .unwrap()
    };
    let response = site.send(preflight("https://games.example.com")).await;
    assert_eq!(
        response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
        "https://games.example.com"
    );
    // Another origin is told the games' origin, which its browser then refuses.
    let response = site.send(preflight("https://evil.example.org")).await;
    assert_eq!(
        response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
        "https://games.example.com"
    );

    // Without games, there's no API.
    let plain = Site::new(site.db.clone());
    let response = plain.send(call("GET", SCORES, "x.y", None)).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn the_guests_page_links_each_game_with_its_token(db: PgPool) {
    let site = Site::with(db, GAMES);
    let (kari, link) = site.invite("Kari", 1).await;
    let party = site.event().await;
    let page = format!("/r/{link}?lang=en");
    // No games offered, no section.
    let html = body(site.get(&page).await).await;
    assert!(!html.contains(&en().games_heading));

    games::offer(&site.db, party.id, &["quiz".into()])
        .await
        .unwrap();
    games::record(&site.db, kari.id, "quiz", 4_560, false)
        .await
        .unwrap();
    let html = body(site.get(&page).await).await;
    let token = site
        .config
        .link_key
        .score_token(kari.id, kari.link_nonce, "quiz");
    let url = format!(
        "https://games.example.com/quiz/#innby=https://fest.example.com&#38;token={token}&#38;lang=en"
    );
    assert!(html.contains(&url), "{html}");
    assert!(
        html.contains("Play Quick quiz") && html.contains("4.56 s"),
        "{html}"
    );
    assert!(!html.contains("Pinball"));

    // The guest's export has their scores.
    let export = json(site.get(&format!("/r/{link}/export")).await).await;
    assert_eq!(export["name"], "Kari");
    assert_eq!(export["scores"][0]["game"], "quiz");
    assert_eq!(export["scores"][0]["score"], 4560);
}
