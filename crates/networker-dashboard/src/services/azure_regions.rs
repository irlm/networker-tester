//! Azure region → IANA timezone mapping for shutdown scheduling.
//!
//! Each region resolves to an IANA zone; `next_shutdown_at` computes the
//! next UTC instant at `local_hour:00:00` in that zone, rolling forward
//! one day if today's slot has already passed.

#![allow(dead_code)] // wired into scheduler in Task 11

use chrono::{DateTime, Datelike, TimeZone, Utc};
use chrono_tz::Tz;

pub fn region_timezone(region: &str) -> Tz {
    match region {
        // US
        "eastus" | "eastus2" | "eastus3" => chrono_tz::US::Eastern,
        "centralus" | "southcentralus" | "northcentralus" => chrono_tz::US::Central,
        "westus" | "westus2" | "westus3" => chrono_tz::US::Pacific,
        "westcentralus" => chrono_tz::US::Mountain,

        // Europe
        "northeurope" => chrono_tz::Europe::Dublin,
        "westeurope" => chrono_tz::Europe::Amsterdam,
        "uksouth" | "ukwest" => chrono_tz::Europe::London,
        "francecentral" | "francesouth" => chrono_tz::Europe::Paris,
        "germanywestcentral" | "germanynorth" => chrono_tz::Europe::Berlin,
        "switzerlandnorth" | "switzerlandwest" => chrono_tz::Europe::Zurich,
        "norwayeast" | "norwaywest" => chrono_tz::Europe::Oslo,
        "swedencentral" => chrono_tz::Europe::Stockholm,
        "polandcentral" => chrono_tz::Europe::Warsaw,
        "italynorth" => chrono_tz::Europe::Rome,
        "spaincentral" => chrono_tz::Europe::Madrid,

        // Asia-Pacific
        "japaneast" | "japanwest" => chrono_tz::Asia::Tokyo,
        "koreacentral" | "koreasouth" => chrono_tz::Asia::Seoul,
        "eastasia" => chrono_tz::Asia::Hong_Kong,
        "southeastasia" => chrono_tz::Asia::Singapore,
        "centralindia" | "southindia" | "westindia" => chrono_tz::Asia::Kolkata,
        "australiaeast" | "australiasoutheast" | "australiacentral" | "australiacentral2" => {
            chrono_tz::Australia::Sydney
        }

        // Americas (non-US)
        "brazilsouth" | "brazilsoutheast" => chrono_tz::America::Sao_Paulo,
        "canadacentral" | "canadaeast" => chrono_tz::America::Toronto,
        "mexicocentral" => chrono_tz::America::Mexico_City,

        // Middle East + Africa
        "uaenorth" | "uaecentral" => chrono_tz::Asia::Dubai,
        "qatarcentral" => chrono_tz::Asia::Qatar,
        "israelcentral" => chrono_tz::Asia::Jerusalem,
        "southafricanorth" | "southafricawest" => chrono_tz::Africa::Johannesburg,

        _ => chrono_tz::UTC,
    }
}

pub fn next_shutdown_at(region: &str, local_hour: i16, now_utc: DateTime<Utc>) -> DateTime<Utc> {
    let tz = region_timezone(region);
    let local_now = now_utc.with_timezone(&tz);
    let hour = local_hour.clamp(0, 23) as u32;

    // Try today at hour:00:00 in the region's local time.
    let today_target = tz.with_ymd_and_hms(
        local_now.year(),
        local_now.month(),
        local_now.day(),
        hour,
        0,
        0,
    );

    // chrono_tz returns LocalResult; take earliest() and fall through on None.
    if let Some(target) = today_target.earliest() {
        if target > local_now {
            return target.with_timezone(&Utc);
        }
    }

    // Today's slot has passed (or was skipped by DST). Roll forward one day.
    let tomorrow_utc = now_utc + chrono::Duration::hours(24);
    let tomorrow_local = tomorrow_utc.with_timezone(&tz);
    let tomorrow_target = tz
        .with_ymd_and_hms(
            tomorrow_local.year(),
            tomorrow_local.month(),
            tomorrow_local.day(),
            hour,
            0,
            0,
        )
        .earliest()
        .unwrap_or_else(|| {
            // Extreme fallback — should never happen.
            (now_utc + chrono::Duration::hours(24)).with_timezone(&tz)
        });

    tomorrow_target.with_timezone(&Utc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn known_regions_resolve() {
        assert_eq!(region_timezone("eastus"), chrono_tz::US::Eastern);
        assert_eq!(region_timezone("westus2"), chrono_tz::US::Pacific);
        assert_eq!(region_timezone("japaneast"), chrono_tz::Asia::Tokyo);
        assert_eq!(region_timezone("uksouth"), chrono_tz::Europe::London);
    }

    #[test]
    fn unknown_region_falls_back_to_utc() {
        assert_eq!(region_timezone("mars-central-1"), chrono_tz::UTC);
    }

    #[test]
    fn next_shutdown_is_in_future() {
        let now = Utc::now();
        let t = next_shutdown_at("eastus", 23, now);
        assert!(t > now, "expected shutdown > now, got {t} vs {now}");
    }

    #[test]
    fn next_shutdown_within_24h() {
        let now = Utc::now();
        let t = next_shutdown_at("eastus", 23, now);
        let delta = t - now;
        assert!(delta <= Duration::hours(25), "expected ≤25h, got {delta:?}");
    }
}
