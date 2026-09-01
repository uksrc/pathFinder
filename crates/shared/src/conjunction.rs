use std::time::Duration;

use regex::Regex;
use reqwest::Client;
use anyhow::{Context, Ok, Result};
use serde::Deserialize;
use roxmltree::Document;
const VO_SPACE_BASE_URL: &str = "https://canfar.cam-preprod.uksrc.org/cavern/nodes/projects";
const IAM_USERINFO_ENDPOINT: &str = "https://ska-iam.stfc.ac.uk/userinfo";
const POSIX_MAPPER_BASE_URL: &str = "https://canfar.cam-preprod.uksrc.org/posix-mapper";

#[derive(Deserialize)]
struct ProfileResponse {
    groups: Vec<String>,
    preferred_username: String,
}

struct PosixInfo {
    uid: u32,
    gid: Option<u32>,
}

struct VOSpaceProperties {
    creator: Option<String>,
    groupwrite: Option<String>,
}

async fn get_user_profile(access_token: &String) -> Result<ProfileResponse> {
    let client = Client::new();
    let response = client
        .get(IAM_USERINFO_ENDPOINT)
        .header("Content-Type", "application/json")
        .header("Authorization", "Bearer ".to_string() + &access_token)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .with_context(|| "Failed to get user profile using Bearer token")?;

    let profile_data: ProfileResponse = response
        .error_for_status()?
        .json()
        .await
        .with_context(|| "Failed to get user profile information")?;

    Ok(profile_data)

}

fn extract_username_from_profile(profile: ProfileResponse) -> Result<String> {
    let input = profile.preferred_username;
    let lower = input.to_lowercase();
    let sanitized: String = lower
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect();

    // Ensure it doesn't start with a number or hyphen
    let mut chars = sanitized.chars();
    let checked = match chars.next() {
        Some(c) if c.is_ascii_digit() || c == '-' => format!("u_{sanitized}"),
        Some(_) => sanitized,
        None => "no_username".to_string(),
    };

    Ok(checked)
}

async fn get_posix_uid_gid(access_token: &String, username: &String) -> Result<PosixInfo> {
    let posix_mapper_url = POSIX_MAPPER_BASE_URL;
    let url = format!("{}/uid?user={}", posix_mapper_url, username);
    let client = Client::new();
    let response = client
        .get(&url)
        .header("Authorization", "Bearer ".to_string() + &access_token)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .with_context(|| format!("Failed to get uid from posix-mapper for user {}", username))?;

    let response_text = response
        .error_for_status()?
        .text()
        .await
        .with_context(|| format!("Failed to get uid and gid fro posix-mapper for user {}", username))?;

    let response_vec: Vec<&str> = response_text.trim().split(":").collect();

    Ok( PosixInfo {
        uid: response_vec[2].parse().unwrap(),
        gid: Some(response_vec[3].parse::<u32>().unwrap()),
    })
}   

async fn get_vospace_properties(access_token: &String, project_name: &String) -> Result<VOSpaceProperties> {
    let url = format!("{}/{}",VO_SPACE_BASE_URL, project_name);
    let client = Client::new();
    let response = client
        .get(&url)
        .header("Authorisarion", "Bearer ".to_string() + &access_token)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .with_context(|| "Failed to query VO Space node")?;

    let response_text = response
        .error_for_status()?
        .text()
        .await
        .with_context(|| "Failed to pqrse VO space response")?;

    let root = Document::parse(&response_text)?
        .root_element();
    let properties = root
        .children()
        .filter(|n| n.has_tag_name("vos:properties"))
        .next()
        .unwrap();

    let vo_space_properties = VOSpaceProperties {
        creator: Some(properties
            .children()
            .find(|n| n.attribute("uri") == Some("ivo://ivoa.net/vospace/core#creator"))
            .and_then(|n| n.text())
            .unwrap()
            .to_string()),
        groupwrite: Some(properties
            .children()
            .find(|n| n.attribute("uri") == Some("ivo://ivoa.net/vospace/core#groupwrite"))
            .and_then(|n| n.text())
            .unwrap()
            .to_string()),
    };

    Ok(vo_space_properties)
    

}

fn extract_group_name_from_groupwrite(groupwrite: String) -> Result<String> {
    let re = Regex::new(r"\?(.+)$").unwrap();
    let group = re.captures_iter(&groupwrite).next().unwrap();
    
    Ok(group[0].split("/").last().unwrap().to_string())
}

fn check_user_group_access(profile: ProfileResponse, project_name: &String) -> Result<bool> {
    let iam_group_name = format!("gateway-projects/{}", project_name.trim());
    let iam_groups = profile.groups;

    let normalized_project = project_name.trim().to_lowercase();
    let mut result = false;
    for group in iam_groups {
        let normalized_group = group.trim().to_lowercase();
        if normalized_group == normalized_project
            || normalized_group == iam_group_name.to_lowercase()
            || normalized_group.ends_with(&format!("/{}",normalized_project))
            || normalized_project.ends_with(&format!("/{}",normalized_group)) {
            result = true;
        }
    }
    Ok(result)

}

fn get_group_id(group_name: String) -> Result<u32> {
    let group = users::get_group_by_name(&group_name).unwrap();
    Ok(group.gid())
}