//! Cloud region → IANA timezone mapping for shutdown scheduling.
//!
//! Each region resolves to an IANA zone; `next_shutdown_at` computes the
//! next UTC instant at `local_hour:00:00` in that zone, rolling forward
//! one day if today's slot has already passed.
//!
//! Provider-aware variants (`_for_provider`) dispatch across Azure, AWS, and
//! GCP region naming conventions.

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

fn aws_region_timezone(region: &str) -> Tz {
    match region {
        "us-east-1" | "us-east-2" => chrono_tz::US::Eastern,
        "us-west-1" | "us-west-2" => chrono_tz::US::Pacific,
        "eu-west-1" => chrono_tz::Europe::Dublin,
        "eu-west-2" => chrono_tz::Europe::London,
        "eu-central-1" => chrono_tz::Europe::Berlin,
        "ap-northeast-1" => chrono_tz::Asia::Tokyo,
        "ap-southeast-1" => chrono_tz::Asia::Singapore,
        "ap-southeast-2" => chrono_tz::Australia::Sydney,
        "sa-east-1" => chrono_tz::America::Sao_Paulo,
        _ => chrono_tz::UTC,
    }
}

fn gcp_region_timezone(region: &str) -> Tz {
    match region {
        "us-central1" | "us-east1" | "us-east4" => chrono_tz::US::Eastern,
        "us-west1" | "us-west2" | "us-west4" => chrono_tz::US::Pacific,
        "europe-west1" | "europe-west4" => chrono_tz::Europe::Amsterdam,
        "europe-west2" => chrono_tz::Europe::London,
        "europe-west3" => chrono_tz::Europe::Berlin,
        "asia-east1" | "asia-east2" => chrono_tz::Asia::Taipei,
        "asia-northeast1" => chrono_tz::Asia::Tokyo,
        "asia-southeast1" => chrono_tz::Asia::Singapore,
        "australia-southeast1" => chrono_tz::Australia::Sydney,
        _ => chrono_tz::UTC,
    }
}

/// Dispatch region → timezone by cloud provider.
///
/// Accepts `"azure"`, `"aws"`, or `"gcp"` as the provider string (case-sensitive).
/// Falls back to UTC for unknown providers or unknown regions within a provider.
pub fn region_timezone_for_provider(provider: &str, region: &str) -> Tz {
    match provider {
        "azure" => region_timezone(region),
        "aws" => aws_region_timezone(region),
        "gcp" => gcp_region_timezone(region),
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

/// Provider-aware variant of [`next_shutdown_at`].
///
/// Computes the next UTC instant at `local_hour:00:00` in the timezone
/// corresponding to `provider` + `region`, rolling forward one day if
/// today's slot has already passed.
pub fn next_shutdown_at_for_provider(
    provider: &str,
    region: &str,
    local_hour: i16,
    now_utc: DateTime<Utc>,
) -> DateTime<Utc> {
    let tz = region_timezone_for_provider(provider, region);
    let local_now = now_utc.with_timezone(&tz);
    let hour = local_hour.clamp(0, 23) as u32;

    let today_target = tz.with_ymd_and_hms(
        local_now.year(),
        local_now.month(),
        local_now.day(),
        hour,
        0,
        0,
    );

    if let Some(target) = today_target.earliest() {
        if target > local_now {
            return target.with_timezone(&Utc);
        }
    }

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
        .unwrap_or_else(|| (now_utc + chrono::Duration::hours(24)).with_timezone(&tz));

    tomorrow_target.with_timezone(&Utc)
}

/// Return a list of commonly-available region identifiers for a given cloud
/// provider. Used by the tester regions endpoint when the project has
/// active `cloud_connection` rows. The lists are static — a real
/// implementation would query the provider's API for the subscription's
/// available regions, but this is sufficient for the MVP.
pub fn regions_for_cloud(provider: &str) -> &'static [&'static str] {
    match provider {
        "azure" => &[
            "eastus",
            "eastus2",
            "westus2",
            "westus3",
            "centralus",
            "southcentralus",
            "northeurope",
            "westeurope",
            "uksouth",
            "francecentral",
            "germanywestcentral",
            "japaneast",
            "koreacentral",
            "southeastasia",
            "australiaeast",
            "brazilsouth",
            "canadacentral",
        ],
        "aws" => &[
            "us-east-1",
            "us-east-2",
            "us-west-1",
            "us-west-2",
            "eu-west-1",
            "eu-west-2",
            "eu-central-1",
            "ap-northeast-1",
            "ap-southeast-1",
            "ap-southeast-2",
            "sa-east-1",
        ],
        "gcp" => &[
            "us-central1",
            "us-east1",
            "us-east4",
            "us-west1",
            "us-west2",
            "europe-west1",
            "europe-west2",
            "europe-west3",
            "europe-west4",
            "asia-east1",
            "asia-northeast1",
            "asia-southeast1",
            "australia-southeast1",
        ],
        _ => &[],
    }
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

    #[test]
    fn aws_known_regions_resolve() {
        assert_eq!(aws_region_timezone("us-east-1"), chrono_tz::US::Eastern);
        assert_eq!(aws_region_timezone("us-east-2"), chrono_tz::US::Eastern);
        assert_eq!(aws_region_timezone("us-west-1"), chrono_tz::US::Pacific);
        assert_eq!(aws_region_timezone("us-west-2"), chrono_tz::US::Pacific);
        assert_eq!(aws_region_timezone("eu-west-1"), chrono_tz::Europe::Dublin);
        assert_eq!(aws_region_timezone("eu-west-2"), chrono_tz::Europe::London);
        assert_eq!(
            aws_region_timezone("eu-central-1"),
            chrono_tz::Europe::Berlin
        );
        assert_eq!(
            aws_region_timezone("ap-northeast-1"),
            chrono_tz::Asia::Tokyo
        );
        assert_eq!(
            aws_region_timezone("ap-southeast-1"),
            chrono_tz::Asia::Singapore
        );
        assert_eq!(
            aws_region_timezone("ap-southeast-2"),
            chrono_tz::Australia::Sydney
        );
        assert_eq!(
            aws_region_timezone("sa-east-1"),
            chrono_tz::America::Sao_Paulo
        );
        assert_eq!(aws_region_timezone("unknown-region"), chrono_tz::UTC);
    }

    #[test]
    fn gcp_known_regions_resolve() {
        assert_eq!(gcp_region_timezone("us-central1"), chrono_tz::US::Eastern);
        assert_eq!(gcp_region_timezone("us-east1"), chrono_tz::US::Eastern);
        assert_eq!(gcp_region_timezone("us-east4"), chrono_tz::US::Eastern);
        assert_eq!(gcp_region_timezone("us-west1"), chrono_tz::US::Pacific);
        assert_eq!(gcp_region_timezone("us-west2"), chrono_tz::US::Pacific);
        assert_eq!(gcp_region_timezone("us-west4"), chrono_tz::US::Pacific);
        assert_eq!(
            gcp_region_timezone("europe-west1"),
            chrono_tz::Europe::Amsterdam
        );
        assert_eq!(
            gcp_region_timezone("europe-west4"),
            chrono_tz::Europe::Amsterdam
        );
        assert_eq!(
            gcp_region_timezone("europe-west2"),
            chrono_tz::Europe::London
        );
        assert_eq!(
            gcp_region_timezone("europe-west3"),
            chrono_tz::Europe::Berlin
        );
        assert_eq!(gcp_region_timezone("asia-east1"), chrono_tz::Asia::Taipei);
        assert_eq!(gcp_region_timezone("asia-east2"), chrono_tz::Asia::Taipei);
        assert_eq!(
            gcp_region_timezone("asia-northeast1"),
            chrono_tz::Asia::Tokyo
        );
        assert_eq!(
            gcp_region_timezone("asia-southeast1"),
            chrono_tz::Asia::Singapore
        );
        assert_eq!(
            gcp_region_timezone("australia-southeast1"),
            chrono_tz::Australia::Sydney
        );
        assert_eq!(gcp_region_timezone("unknown-region"), chrono_tz::UTC);
    }

    #[test]
    fn provider_dispatch_routes_correctly() {
        assert_eq!(
            region_timezone_for_provider("azure", "eastus"),
            chrono_tz::US::Eastern
        );
        assert_eq!(
            region_timezone_for_provider("aws", "us-east-1"),
            chrono_tz::US::Eastern
        );
        assert_eq!(
            region_timezone_for_provider("gcp", "us-central1"),
            chrono_tz::US::Eastern
        );
        assert_eq!(
            region_timezone_for_provider("unknown", "us-east-1"),
            chrono_tz::UTC
        );
    }

    #[test]
    fn next_shutdown_for_provider_is_in_future() {
        let now = Utc::now();
        let t = next_shutdown_at_for_provider("aws", "us-east-1", 23, now);
        assert!(t > now, "expected shutdown > now, got {t} vs {now}");
    }
}
