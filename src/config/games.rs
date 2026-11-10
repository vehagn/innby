//! The minigames a host can offer on an event's page: `INNBY_GAMES`, each game's id,
//! whether it scores points or time, whether its own server sends the scores, and its
//! name; and `INNBY_GAMES_URL`, the origin they're served from, under `/{id}/`. See
//! docs/games.md.
//!
//! The games are JavaScript, so they live on an origin of their own: on the site's,
//! a game could read a guest's pages, and their link with them.

use anyhow::{Result, anyhow, bail, ensure};

use super::env::Env;

/// How a game's scores rank.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ScoreKind {
    /// Points: the most wins.
    Points,
    /// Milliseconds taken: the fewest wins.
    Time,
}

impl ScoreKind {
    pub fn higher_wins(self) -> bool {
        self == ScoreKind::Points
    }

    /// A score as the pages show it: points as they are, time in seconds to a
    /// hundredth.
    pub fn show(self, score: i64, t: &crate::i18n::Messages) -> String {
        match self {
            ScoreKind::Points => score.to_string(),
            ScoreKind::Time => {
                let hundredths = score / 10;
                let seconds = format!(
                    "{}{}{:02}",
                    hundredths / 100,
                    t.decimal_mark,
                    hundredths % 100
                );
                crate::i18n::fill(&t.seconds, &[("n", &seconds)])
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Game {
    /// Its path under `INNBY_GAMES_URL`, and how events and scores name it.
    pub id: String,
    pub kind: ScoreKind,
    /// Whether the game's own server sends its scores, through the internal API with
    /// the game's key, having worked them out itself. Browsers can't send them then,
    /// so a score can't be made up with a request.
    pub server: bool,
    /// As the pages show it, in every language.
    pub name: String,
}

#[derive(Debug, Clone, Default)]
pub struct Games {
    /// The games' origin, with any path, and no slash at the end:
    /// `https://games.example.com`.
    pub url: String,
    pub list: Vec<Game>,
}

/// The longest game id, so ids stay a path segment and a column of their own.
pub const MAX_ID_LEN: usize = 40;

impl Games {
    pub fn find(&self, id: &str) -> Option<&Game> {
        self.list.iter().find(|game| game.id == id)
    }

    /// Where a guest plays a game. The fragment, which browsers never send to a server,
    /// tells the game where Innby is, the guest's score token and their language.
    pub fn play_url(&self, game: &Game, site: &str, token: &str, lang: &str) -> String {
        format!(
            "{}/{}/#innby={site}&token={token}&lang={lang}",
            self.url, game.id
        )
    }

    /// The origin scores may come from, for CORS: the URL without its path.
    pub fn origin(&self) -> Option<&str> {
        let (scheme, rest) = self.url.split_once("://")?;
        let host = rest.split('/').next()?;
        Some(&self.url[..scheme.len() + 3 + host.len()])
    }
}

impl<F: Fn(&str) -> Option<String>> Env<F> {
    /// `INNBY_GAMES`: `id:points|time:Name` each, apart by lines or `;`, as in
    /// `pinball:points:Pinball; quiz:time:Quick quiz`, with `server:` before the name
    /// for a game whose server sends the scores: `snake:points:server:Snake`. Needs `INNBY_GAMES_URL`, on an
    /// origin other than the site's.
    pub(super) fn games(&self, base_url: &str) -> Result<Games> {
        let Some(list) = self.optional("INNBY_GAMES") else {
            return Ok(Games::default());
        };
        let url = self
            .optional("INNBY_GAMES_URL")
            .ok_or_else(|| anyhow!("INNBY_GAMES_URL must be set for INNBY_GAMES"))?;
        let url = url.trim().trim_end_matches('/').to_owned();
        let parsed = url::Url::parse(&url)
            .ok()
            .filter(|parsed| matches!(parsed.scheme(), "http" | "https") && parsed.host().is_some())
            .filter(|parsed| parsed.query().is_none() && parsed.fragment().is_none())
            .ok_or_else(|| {
                anyhow!("INNBY_GAMES_URL: expected an address like https://games.example.com, got {url:?}")
            })?;
        let site = url::Url::parse(base_url)?;
        ensure!(
            parsed.origin() != site.origin(),
            "INNBY_GAMES_URL: the games need an origin of their own, such as \
             https://games.example.com: on the site's, a game could read guests' links"
        );
        let mut games = Games {
            url,
            list: Vec::new(),
        };
        for entry in list
            .split([';', '\n'])
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
        {
            let game = parse_game(entry).map_err(|err| anyhow!("INNBY_GAMES: {err}"))?;
            ensure!(
                games.find(&game.id).is_none(),
                "INNBY_GAMES: there's already a game {}",
                game.id
            );
            games.list.push(game);
        }
        Ok(games)
    }
}

fn parse_game(entry: &str) -> Result<Game> {
    let mut fields = entry.splitn(3, ':').map(str::trim);
    let (Some(id), Some(kind), Some(rest)) = (fields.next(), fields.next(), fields.next()) else {
        bail!("expected id:points|time:Name, as pinball:points:Pinball, got {entry:?}");
    };
    let (server, name) = match rest.split_once(':') {
        Some(("server", name)) => (true, name.trim()),
        _ => (false, rest),
    };
    ensure!(
        (1..=MAX_ID_LEN).contains(&id.len())
            && id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
        "{id:?}: a game's id is up to {MAX_ID_LEN} letters a to z, digits and -"
    );
    let kind = match kind {
        "points" => ScoreKind::Points,
        "time" => ScoreKind::Time,
        other => bail!("{id}: a game scores points or time, not {other:?}"),
    };
    ensure!(
        !name.is_empty() && name.chars().count() <= 100 && !name.contains(char::is_control),
        "{id}: a game's name is one line of up to 100 characters"
    );
    Ok(Game {
        id: id.to_owned(),
        kind,
        server,
        name: name.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn games(pairs: &[(&str, &str)]) -> Result<Games> {
        let env: HashMap<&str, &str> = pairs.iter().copied().collect();
        Env(|key: &str| env.get(key).map(|value| value.to_string()))
            .games("https://fest.example.com")
    }

    #[test]
    fn reads_the_games_and_where_they_are() {
        assert!(games(&[]).unwrap().list.is_empty());
        let found = games(&[
            ("INNBY_GAMES_URL", "https://games.example.com/"),
            (
                "INNBY_GAMES",
                "pinball:points:Pinball; \n quiz:time:Quick quiz: round 1\n",
            ),
        ])
        .unwrap();
        assert_eq!(found.url, "https://games.example.com");
        assert_eq!(found.origin(), Some("https://games.example.com"));
        assert_eq!(found.list.len(), 2);
        let quiz = found.find("quiz").unwrap();
        assert!(!quiz.server);
        assert_eq!(
            (quiz.kind, quiz.name.as_str()),
            (ScoreKind::Time, "Quick quiz: round 1")
        );
        assert_eq!(
            found.play_url(quiz, "https://fest.example.com", "a.b", "en"),
            "https://games.example.com/quiz/#innby=https://fest.example.com&token=a.b&lang=en"
        );
        // Under a path of its own.
        let under = games(&[
            ("INNBY_GAMES_URL", "https://example.org/games"),
            ("INNBY_GAMES", "quiz:time:Quiz"),
        ])
        .unwrap();
        assert_eq!(under.origin(), Some("https://example.org"));
    }

    #[test]
    fn a_game_can_have_its_server_send_the_scores() {
        let found = games(&[
            ("INNBY_GAMES_URL", "https://games.example.com"),
            (
                "INNBY_GAMES",
                "snake:points:server:Snake; quiz:time:server: the quiz",
            ),
        ])
        .unwrap();
        let snake = found.find("snake").unwrap();
        assert!(snake.server && snake.name == "Snake");
        let quiz = found.find("quiz").unwrap();
        assert!(quiz.server && quiz.name == "the quiz");
    }

    #[test]
    fn shows_points_and_seconds() {
        let (en, nb) = (
            crate::i18n::Lang::EN.messages(),
            crate::i18n::Lang::NB.messages(),
        );
        assert_eq!(ScoreKind::Points.show(1200, en), "1200");
        assert_eq!(ScoreKind::Time.show(12_345, en), "12.34 s");
        assert_eq!(ScoreKind::Time.show(905, nb), "0,90 s");
    }

    #[test]
    fn refuses_what_cant_work() {
        let err = |url: &str, list: &str| {
            let pairs = [("INNBY_GAMES_URL", url), ("INNBY_GAMES", list)];
            format!("{:#}", games(&pairs).unwrap_err())
        };
        let url = "https://games.example.com";
        assert!(err("", "quiz:time:Quiz").contains("INNBY_GAMES_URL must be set"));
        assert!(
            err("https://fest.example.com/games", "quiz:time:Quiz").contains("origin of their own")
        );
        assert!(err("games.example.com", "quiz:time:Quiz").contains("expected an address"));
        assert!(err(url, "quiz:Quiz").contains("expected id:points|time:Name"));
        assert!(err(url, "Quiz:time:Quiz").contains("a game's id"));
        assert!(err(url, "quiz:score:Quiz").contains("points or time"));
        assert!(err(url, "quiz:time:Quiz; quiz:points:Again").contains("already a game quiz"));
    }
}
