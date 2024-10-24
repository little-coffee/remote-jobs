use futures::{stream, StreamExt};
use itertools::Itertools;
use regex::Regex;
use reqwest::redirect::Policy;
use reqwest::Client;
use std::borrow::Cow;
use std::collections::HashSet;
use std::fs::File;
use std::io::prelude::*;
use std::ops::Deref;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::time::Instant;

const PARALLEL_REQUESTS: usize = 10;

fn remove_url_protocol(url: &str) -> Cow<'_, str> {
    let re = Regex::new(r"^(https?)://").unwrap();
    re.replace(url, "")
}

fn remove_www(url: &str) -> Cow<'_, str> {
    let re = Regex::new(r"(www\.)").unwrap();
    re.replace(url, "")
}

fn sanitize_url(url: &str) -> String {
    remove_www(remove_url_protocol(url).deref()).into_owned()
}

fn build_url_variations(url: &str) -> Vec<String> {
    let partial_url = sanitize_url(url);
    // NOTE: order matters here
    let mut variations = vec![
        format!("https://{}", partial_url),
        format!("https://www.{}", partial_url),
        format!("http://{}", partial_url),
        format!("http://www.{}", partial_url),
    ];
    if !variations.contains(&url.to_string()) {
        variations.push(url.to_owned()); // AS IS
    }
    variations
}

async fn hit_url(url: &str) -> Result<String, reqwest::Error> {
    println!("hitting {}", url);
    let now = Instant::now();
    let final_url = Arc::new(Mutex::new(url.to_owned()));
    let client = Client::builder()
        .redirect(Policy::custom({
            let final_url = Arc::clone(&final_url);
            move |attempt| {
                let new_url = attempt.url().to_string();
                println!(
                    "redirecting to {} from {}",
                    new_url,
                    final_url.lock().unwrap().as_str()
                );
                *final_url.lock().unwrap() = new_url;
                attempt.follow() // Follow the redirect
            }
        }))
        .timeout(Duration::from_secs(120))
        .build()?;

    client.get(url).send().await?;
    println!("{} secs for {}", now.elapsed().as_secs(), url);
    let final_url = final_url.lock().unwrap().to_owned();
    Ok(final_url)
}

struct CheckedUrl {
    is_valid: bool,
    url: String,
}

async fn check_url(url: &str) -> CheckedUrl {
    let variations = build_url_variations(url);
    for variation in variations {
        if let Ok(final_url) = hit_url(&variation).await {
            return CheckedUrl {
                is_valid: true,
                url: final_url,
            };
        }
    }
    println!("invalid - {}", url);
    CheckedUrl {
        is_valid: false,
        url: url.to_owned(),
    }
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let mut file = File::open("job-links.txt")?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    let urls = contents
        .split('\n')
        .into_iter()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_owned())
            }
        })
        .unique()
        .collect::<Vec<String>>();

    let results = stream::iter(urls)
        .map(|url| tokio::spawn(async move { check_url(&url).await }))
        .buffer_unordered(PARALLEL_REQUESTS);

    let valid_urls = HashSet::new();
    let invalid_urls = HashSet::new();

    struct UrlSets {
        valid_urls: HashSet<String>,
        invalid_urls: HashSet<String>,
    }

    let UrlSets {
        valid_urls,
        invalid_urls,
    } = results
        .fold(
            UrlSets {
                valid_urls,
                invalid_urls,
            },
            |mut acc, result| async {
                let UrlSets {
                    valid_urls,
                    invalid_urls,
                } = &mut acc;

                match result {
                    Ok(checked) => {
                        if checked.is_valid {
                            valid_urls.insert(checked.url);
                        } else {
                            invalid_urls.insert(checked.url);
                        }
                    }
                    Err(e) => eprintln!("Got a tokio::JoinError: {}", e),
                }
                acc
            },
        )
        .await;

    let mut output_file = File::create("output.txt")?;
    output_file.write_all(
        format!(
            "valid urls:\n{}\n\ninvalid urls:\n{}",
            valid_urls.iter().sorted().dedup().join("\n"),
            invalid_urls.iter().sorted().dedup().join("\n")
        )
        .as_bytes(),
    )?;

    println!(
        "valid urls:\n{}\n\ninvalid urls:\n{}",
        valid_urls.iter().sorted().dedup().join("\n"),
        invalid_urls.iter().sorted().dedup().join("\n")
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_url() {
        assert_eq!(sanitize_url("https://www.google.com"), "google.com");
        assert_eq!(sanitize_url("https://www.google.com/"), "google.com/");
        assert_eq!(
            sanitize_url("https://www.google.com/search?q=rust&p=1"),
            "google.com/search?q=rust&p=1"
        );
    }

    #[test]
    fn test_build_url_variations() {
        assert_eq!(
            build_url_variations("https://drupaljedi.com"),
            vec![
                "https://drupaljedi.com",
                "https://www.drupaljedi.com",
                "http://drupaljedi.com",
                "http://www.drupaljedi.com",
            ]
        );
        assert_eq!(
            build_url_variations("https://www.google.com/search?q=rust&p=1"),
            vec![
                "https://google.com/search?q=rust&p=1",
                "https://www.google.com/search?q=rust&p=1",
                "http://google.com/search?q=rust&p=1",
                "http://www.google.com/search?q=rust&p=1",
            ]
        );
    }
}
