//! Adaptador do que o Claude Code passa à linha de estado (`rate_limits`) para o nosso
//! `ClaudeUsage`, e a pasta onde o último valor fica guardado.
//!
//! Porquê um adaptador: o protocolo da island usa nomes e unidades próprios (`used_pct`,
//! `resets_at` em segundos desde 1970). Se o Claude Code mudar o JSON, só este ficheiro muda.
//!
//! Formato esperado (cada janela pode faltar; os números podem vir como inteiros ou decimais):
//!
//! ```json
//! {"rate_limits":{"five_hour":{"used_percentage":29,"resets_at":1794366000},
//!                 "seven_day":{"used_percentage":55,"resets_at":"2026-11-17T09:00:00Z"}}}
//! ```

use crate::{ClaudeUsage, UsageWindow};
use serde_json::Value;
use std::path::PathBuf;

/// Nome do ficheiro com o último valor.
pub const USAGE_FILE: &str = "usage.json";

/// Pasta de estado da island: `%LOCALAPPDATA%\dynamic-island` (é onde vivem os binários).
/// `ISLAND_STATE_DIR` substitui-a (testes e desenvolvimento).
pub fn state_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("ISLAND_STATE_DIR") {
        return Some(PathBuf::from(dir));
    }
    std::env::var_os("LOCALAPPDATA").map(|base| PathBuf::from(base).join("dynamic-island"))
}

/// Extrai os limites do JSON da linha de estado. `None` se não há `rate_limits` ou nenhuma das
/// duas janelas (antes da 1.ª resposta da sessão, ou em planos sem limite).
pub fn usage_from_status_line(payload: &Value) -> Option<ClaudeUsage> {
    let limits = payload.get("rate_limits")?;
    let usage = ClaudeUsage { five_hour: window(limits.get("five_hour")), seven_day: window(limits.get("seven_day")) };
    (usage.five_hour.is_some() || usage.seven_day.is_some()).then_some(usage)
}

fn window(v: Option<&Value>) -> Option<UsageWindow> {
    let v = v?;
    // `used_percentage` na linha de estado; `utilization` na API; aceita-se qualquer.
    let pct = ["used_percentage", "used_pct", "utilization"].iter().find_map(|k| v.get(*k)?.as_f64())?;
    if !pct.is_finite() {
        return None;
    }
    let resets_at = match v.get("resets_at") {
        Some(Value::Number(n)) => n.as_f64().map(epoch_seconds),
        Some(Value::String(s)) => rfc3339_to_epoch(s),
        _ => None,
    };
    Some(UsageWindow { used_pct: pct.clamp(0.0, 100.0) as f32, resets_at })
}

/// Segundos desde 1970; um valor em milissegundos converte-se.
fn epoch_seconds(n: f64) -> u64 {
    let n = n.max(0.0);
    (if n > 99_999_999_999.0 { n / 1000.0 } else { n }) as u64
}

/// `2026-11-17T09:00:00Z`, `...00.123Z` ou com `+01:00`, para segundos desde 1970.
fn rfc3339_to_epoch(s: &str) -> Option<u64> {
    let (date, rest) = s.split_once(['T', 't', ' '])?;
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let time_end = rest.find(['Z', 'z', '+', '-']).unwrap_or(rest.len());
    let (clock, zone) = rest.split_at(time_end);
    let mut t = clock.split(':');
    let h: i64 = t.next()?.parse().ok()?;
    let min: i64 = t.next()?.parse().ok()?;
    let sec: f64 = t.next().map_or(Some(0.0), |p| p.parse().ok())?;
    let offset = match zone.as_bytes().first() {
        Some(b'+') | Some(b'-') => {
            let sign = if zone.starts_with('-') { -1 } else { 1 };
            let (oh, om) = zone[1..].split_once(':').unwrap_or((&zone[1..], "0"));
            sign * (oh.parse::<i64>().ok()? * 3600 + om.parse::<i64>().ok()? * 60)
        }
        _ => 0,
    };
    // Dias desde 1970 (algoritmo de Howard Hinnant, calendário gregoriano).
    let (y, m) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + h * 3600 + min * 60 + sec as i64 - offset).ok()
}

/// "5h 29% | 7d 55%" para a linha de estado do Claude Code (vazio sem janelas).
pub fn status_line_text(u: &ClaudeUsage) -> String {
    let part = |label: &str, w: Option<UsageWindow>| w.map(|w| format!("{label} {:.0}%", w.used_pct));
    [part("5h", u.five_hour), part("7d", u.seven_day)].into_iter().flatten().collect::<Vec<_>>().join(" | ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn full_payload_with_both_windows() {
        // O que o Claude Code envia de verdade: muitas outras chaves ao lado de `rate_limits`.
        let v = json!({
            "session_id": "abc", "cwd": "C:\\p", "model": {"id": "claude-opus-5-5"},
            "context_window": {"size": 1000000},
            "rate_limits": {
                "five_hour": {"used_percentage": 29, "resets_at": 1794366000},
                "seven_day": {"used_percentage": 55.4, "resets_at": 1794488400}
            }
        });
        let u = usage_from_status_line(&v).unwrap();
        assert_eq!(u.five_hour, Some(UsageWindow { used_pct: 29.0, resets_at: Some(1_794_366_000) }));
        assert_eq!(u.seven_day, Some(UsageWindow { used_pct: 55.4, resets_at: Some(1_794_488_400) }));
        assert_eq!(status_line_text(&u), "5h 29% | 7d 55%");
    }

    #[test]
    fn without_rate_limits_there_is_nothing_to_send() {
        assert_eq!(usage_from_status_line(&json!({"model": {"id": "x"}, "cwd": "C:\\p"})), None);
        assert_eq!(usage_from_status_line(&json!({"rate_limits": null})), None);
        assert_eq!(usage_from_status_line(&json!({"rate_limits": {}})), None);
        assert_eq!(usage_from_status_line(&json!([])), None);
        assert_eq!(usage_from_status_line(&json!("texto")), None);
    }

    #[test]
    fn only_one_window_is_enough() {
        let only_week = json!({"rate_limits": {"seven_day": {"used_percentage": 80}}});
        let u = usage_from_status_line(&only_week).unwrap();
        assert_eq!(u.five_hour, None);
        assert_eq!(u.seven_day, Some(UsageWindow { used_pct: 80.0, resets_at: None }));
        assert_eq!(status_line_text(&u), "7d 80%");

        let only_five = json!({"rate_limits": {"five_hour": {"used_percentage": 3, "resets_at": 10}, "seven_day": null}});
        let u = usage_from_status_line(&only_five).unwrap();
        assert_eq!(u.seven_day, None);
        assert_eq!(status_line_text(&u), "5h 3%");
    }

    #[test]
    fn a_window_without_percentage_is_dropped() {
        let v = json!({"rate_limits": {"five_hour": {"resets_at": 5}, "seven_day": {"used_percentage": 1}}});
        let u = usage_from_status_line(&v).unwrap();
        assert_eq!(u.five_hour, None);
        assert!(u.seven_day.is_some());
        assert_eq!(usage_from_status_line(&json!({"rate_limits": {"five_hour": {"resets_at": 5}}})), None);
    }

    #[test]
    fn percentages_are_clamped_and_alternative_names_work() {
        let v = json!({"rate_limits": {"five_hour": {"used_percentage": 250}, "seven_day": {"utilization": -3.0}}});
        let u = usage_from_status_line(&v).unwrap();
        assert_eq!(u.five_hour.unwrap().used_pct, 100.0);
        assert_eq!(u.seven_day.unwrap().used_pct, 0.0);
    }

    #[test]
    fn reset_times_accept_seconds_milliseconds_and_iso() {
        let at = |v: Value| usage_from_status_line(&json!({"rate_limits": {"five_hour": {"used_percentage": 1, "resets_at": v}}})).unwrap().five_hour.unwrap().resets_at;
        assert_eq!(at(json!(1794366000)), Some(1_794_366_000));
        assert_eq!(at(json!(1794366000000u64)), Some(1_794_366_000));
        assert_eq!(at(json!("2026-11-10T00:00:00Z")), Some(1_794_268_800));
        assert_eq!(at(json!("2026-11-10T01:00:00+01:00")), Some(1_794_268_800));
        assert_eq!(at(json!("2026-11-10T00:00:00.250Z")), Some(1_794_268_800));
        assert_eq!(at(json!("1970-01-01T00:00:00Z")), Some(0));
        assert_eq!(at(json!("lixo")), None);
        assert_eq!(at(json!(null)), None);
    }

    #[test]
    fn status_line_text_is_empty_without_windows() {
        assert_eq!(status_line_text(&ClaudeUsage::default()), "");
    }
}
