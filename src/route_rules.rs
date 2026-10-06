//! Правила маршрута по сайтам: что идёт через выход в сети, а что напрямую («ютюб через зарубежный выход, российские сайты —
//! как обычно»). Весь трафик в обход незачем: так медленнее и нагружает чужие выходы без причины.
//!
//! Правила — в `route_rules.json` папки данных (`{"via": [...], "direct": [...], "default": "direct"|"via"}`); нет файла —
//! встроенный список известных заблокированных сервисов, остальное напрямую. Совпадение — по имени сайта, которое прислало
//! приложение (SOCKS5 с именем; если пришёл только IP-адрес, применяется `default`): `example.com` подходит и самому
//! `example.com`, и `www.example.com`, но не `badexample.com`. «direct» сильнее «via».
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq)]
pub enum Route {
    /// через выход в сети (по выбранному режиму: быстро или анонимно)
    Via,
    /// напрямую с этого компьютера (только адреса в интернете)
    Direct,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Rules {
    pub via: Vec<String>,
    pub direct: Vec<String>,
    pub default: String,
}

const BUILTIN_VIA: &[&str] = &[
    "youtube.com", "youtu.be", "googlevideo.com", "ytimg.com", "ggpht.com", "youtube-nocookie.com", "google.com", "gstatic.com", "googleapis.com",
    "googleusercontent.com", "gmail.com", "telegram.org", "t.me",
    "instagram.com", "cdninstagram.com", "facebook.com", "fbcdn.net", "twitter.com", "x.com", "twimg.com", "linkedin.com", "discord.com",
    "discordapp.com", "spotify.com", "scdn.co", "medium.com", "notion.so", "github.com", "githubusercontent.com",
];

impl Default for Rules {
    fn default() -> Self {
        Rules { via: BUILTIN_VIA.iter().map(|s| s.to_string()).collect(), direct: vec![], default: "direct".into() }
    }
}

fn matches(host: &str, pattern: &str) -> bool {
    let p = pattern.trim().trim_start_matches("*.").trim_start_matches('.').to_ascii_lowercase();
    !p.is_empty() && (host == p || host.ends_with(&format!(".{p}")))
}

impl Rules {
    pub fn load() -> Rules {
        std::fs::read_to_string(crate::util::data_dir::data_dir().join("route_rules.json")).ok().and_then(|s| serde_json::from_str::<Rules>(&s).ok()).unwrap_or_default()
    }

    pub fn decide(&self, host: &str) -> Route {
        let h = host.trim().trim_end_matches('.').to_ascii_lowercase();
        if self.direct.iter().any(|p| matches(&h, p)) {
            return Route::Direct;
        }
        if self.via.iter().any(|p| matches(&h, p)) {
            return Route::Via;
        }
        if self.default == "via" { Route::Via } else { Route::Direct }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_site_matches_itself_and_its_subdomains_but_not_look_alikes_and_direct_wins() {
        let r = Rules::default();
        assert_eq!(r.decide("youtube.com"), Route::Via);
        assert_eq!(r.decide("www.YouTube.com."), Route::Via);
        assert_eq!(r.decide("rr3---sn-x.googlevideo.com"), Route::Via);
        assert_eq!(r.decide("badyoutube.com"), Route::Direct, "not a subdomain");
        assert_eq!(r.decide("youtube.com.evil.ru"), Route::Direct);
        assert_eq!(r.decide("yandex.ru"), Route::Direct);
        assert_eq!(r.decide("93.184.216.34"), Route::Direct, "only an IP address: the default");
        let custom: Rules = serde_json::from_str(r#"{"via": ["example.org", "*.cdn.net"], "direct": ["api.example.org"], "default": "via"}"#).unwrap();
        assert_eq!(custom.decide("api.example.org"), Route::Direct, "direct is stronger");
        assert_eq!(custom.decide("x.example.org"), Route::Via);
        assert_eq!(custom.decide("a.cdn.net"), Route::Via);
        assert_eq!(custom.decide("anything.else"), Route::Via, "default via");
        let partial: Rules = serde_json::from_str(r#"{"direct": ["a.ru"]}"#).unwrap();
        assert_eq!(partial.decide("youtube.com"), Route::Via, "missing keys fall back to built-ins");
    }
}
