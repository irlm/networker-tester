/// VM size tier normalization and region validation for multi-cloud provisioning.

/// Resolve a human-friendly tier name (small/medium/large) to the cloud-specific
/// VM size string.  If `tier` is not a recognized alias, it is passed through
/// unchanged so callers can supply an exact cloud-native size like
/// `Standard_D8s_v3` directly.
pub fn resolve_vm_size<'a>(cloud: &str, tier: &'a str) -> &'a str {
    match (cloud, tier) {
        // Azure
        ("azure", "small") => "Standard_B2s",
        ("azure", "medium") => "Standard_D2s_v3",
        ("azure", "large") => "Standard_D4s_v3",
        // AWS
        ("aws", "small") => "t3.small",
        ("aws", "medium") => "t3.medium",
        ("aws", "large") => "m5.xlarge",
        // GCP
        ("gcp", "small") => "e2-small",
        ("gcp", "medium") => "n2-standard-2",
        ("gcp", "large") => "n2-standard-4",
        // Pass-through for exact cloud-specific names
        _ => tier,
    }
}

/// Hardcoded list of common regions per cloud provider.
const AZURE_REGIONS: &[&str] = &[
    "eastus",
    "eastus2",
    "westus",
    "westus2",
    "westus3",
    "centralus",
    "northcentralus",
    "southcentralus",
    "westeurope",
    "northeurope",
    "uksouth",
    "ukwest",
    "canadacentral",
    "canadaeast",
    "australiaeast",
    "australiasoutheast",
    "japaneast",
    "japanwest",
    "southeastasia",
    "eastasia",
    "brazilsouth",
    "koreacentral",
    "koreasouth",
    "francecentral",
    "germanywestcentral",
    "switzerlandnorth",
    "norwayeast",
    "swedencentral",
    "qatarcentral",
    "uaenorth",
    "southafricanorth",
    "centralindia",
];

const AWS_REGIONS: &[&str] = &[
    "us-east-1",
    "us-east-2",
    "us-west-1",
    "us-west-2",
    "ca-central-1",
    "eu-west-1",
    "eu-west-2",
    "eu-west-3",
    "eu-central-1",
    "eu-central-2",
    "eu-north-1",
    "eu-south-1",
    "ap-southeast-1",
    "ap-southeast-2",
    "ap-northeast-1",
    "ap-northeast-2",
    "ap-northeast-3",
    "ap-south-1",
    "ap-east-1",
    "sa-east-1",
    "me-south-1",
    "af-south-1",
    "me-central-1",
    "il-central-1",
];

const GCP_REGIONS: &[&str] = &[
    "us-central1",
    "us-east1",
    "us-east4",
    "us-east5",
    "us-south1",
    "us-west1",
    "us-west2",
    "us-west3",
    "us-west4",
    "europe-west1",
    "europe-west2",
    "europe-west3",
    "europe-west4",
    "europe-west6",
    "europe-north1",
    "europe-central2",
    "asia-east1",
    "asia-east2",
    "asia-northeast1",
    "asia-northeast2",
    "asia-northeast3",
    "asia-south1",
    "asia-south2",
    "asia-southeast1",
    "asia-southeast2",
    "australia-southeast1",
    "australia-southeast2",
    "southamerica-east1",
    "northamerica-northeast1",
    "northamerica-northeast2",
    "me-west1",
    "me-central1",
    "africa-south1",
];

/// Check whether `region` is a known region for the given cloud provider.
///
/// For GCP, both region (e.g. `us-central1`) and zone (e.g. `us-central1-a`)
/// formats are accepted — the zone suffix is stripped before matching.
pub fn validate_region(cloud: &str, region: &str) -> bool {
    match cloud {
        "azure" => AZURE_REGIONS.contains(&region),
        "aws" => AWS_REGIONS.contains(&region),
        "gcp" => {
            // Accept either region or zone (region + "-a"/"-b"/"-c"/etc.)
            let base = strip_gcp_zone_suffix(region);
            GCP_REGIONS.contains(&base)
        }
        _ => false,
    }
}

/// Strip the trailing zone letter from a GCP zone string (e.g. `us-central1-a` -> `us-central1`).
/// If the input doesn't look like a zone (no trailing `-<letter>`), it is returned unchanged.
fn strip_gcp_zone_suffix(zone: &str) -> &str {
    // GCP zones end in "-<single letter>", e.g. "us-central1-a"
    if zone.len() >= 3 {
        let bytes = zone.as_bytes();
        let last = bytes[bytes.len() - 1];
        let sep = bytes[bytes.len() - 2];
        if sep == b'-' && last.is_ascii_lowercase() {
            return &zone[..zone.len() - 2];
        }
    }
    zone
}

/// Return the default region for a cloud provider.
pub fn default_region(cloud: &str) -> &'static str {
    match cloud {
        "azure" => "eastus",
        "aws" => "us-east-1",
        "gcp" => "us-central1-a",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── resolve_vm_size ────────────────────────────────────────────

    #[test]
    fn tier_small() {
        assert_eq!(resolve_vm_size("azure", "small"), "Standard_B2s");
        assert_eq!(resolve_vm_size("aws", "small"), "t3.small");
        assert_eq!(resolve_vm_size("gcp", "small"), "e2-small");
    }

    #[test]
    fn tier_medium() {
        assert_eq!(resolve_vm_size("azure", "medium"), "Standard_D2s_v3");
        assert_eq!(resolve_vm_size("aws", "medium"), "t3.medium");
        assert_eq!(resolve_vm_size("gcp", "medium"), "n2-standard-2");
    }

    #[test]
    fn tier_large() {
        assert_eq!(resolve_vm_size("azure", "large"), "Standard_D4s_v3");
        assert_eq!(resolve_vm_size("aws", "large"), "m5.xlarge");
        assert_eq!(resolve_vm_size("gcp", "large"), "n2-standard-4");
    }

    #[test]
    fn tier_passthrough() {
        // Exact cloud-specific names pass through unchanged
        assert_eq!(
            resolve_vm_size("azure", "Standard_D8s_v3"),
            "Standard_D8s_v3"
        );
        assert_eq!(resolve_vm_size("aws", "c5.2xlarge"), "c5.2xlarge");
        assert_eq!(resolve_vm_size("gcp", "c2-standard-8"), "c2-standard-8");
        // Unknown cloud also passes through
        assert_eq!(resolve_vm_size("digitalocean", "small"), "small");
    }

    // ── validate_region ────────────────────────────────────────────

    #[test]
    fn azure_regions() {
        assert!(validate_region("azure", "eastus"));
        assert!(validate_region("azure", "westeurope"));
        assert!(!validate_region("azure", "us-east-1"));
        assert!(!validate_region("azure", "made-up"));
    }

    #[test]
    fn aws_regions() {
        assert!(validate_region("aws", "us-east-1"));
        assert!(validate_region("aws", "eu-west-1"));
        assert!(!validate_region("aws", "eastus"));
        assert!(!validate_region("aws", "fake-region"));
    }

    #[test]
    fn gcp_regions() {
        assert!(validate_region("gcp", "us-central1"));
        assert!(validate_region("gcp", "europe-west1"));
        assert!(!validate_region("gcp", "us-east-1"));
    }

    #[test]
    fn gcp_zone_accepted() {
        assert!(validate_region("gcp", "us-central1-a"));
        assert!(validate_region("gcp", "europe-west1-b"));
        assert!(validate_region("gcp", "asia-east1-c"));
        assert!(!validate_region("gcp", "us-central1-1")); // digit, not letter
    }

    #[test]
    fn unknown_cloud_rejected() {
        assert!(!validate_region("digitalocean", "nyc1"));
    }

    // ── default_region ─────────────────────────────────────────────

    #[test]
    fn defaults() {
        assert_eq!(default_region("azure"), "eastus");
        assert_eq!(default_region("aws"), "us-east-1");
        assert_eq!(default_region("gcp"), "us-central1-a");
        assert_eq!(default_region("other"), "unknown");
    }

    // ── strip_gcp_zone_suffix ──────────────────────────────────────

    #[test]
    fn strip_zone() {
        assert_eq!(strip_gcp_zone_suffix("us-central1-a"), "us-central1");
        assert_eq!(strip_gcp_zone_suffix("us-central1"), "us-central1");
        assert_eq!(strip_gcp_zone_suffix("ab"), "ab"); // too short
    }
}
