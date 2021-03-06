use crate::helpers::between;

fn parse_cas_response(body: &str) -> Option<&str> {
    if body.contains("<cas:authenticationSuccess>") {
        between(body, "<cas:user>", "</")
    } else {
        None
    }
}

async fn http_get(url: &str) -> reqwest::Result<(reqwest::StatusCode, String)> {
    let res = reqwest::get(url).await?;
    Ok((res.status(), res.text().await?))
}

pub async fn validate_ticket(cas_prefix_url: &str, service: &str, ticket: &str) -> Result<String, String> {
    let url = format!("{}/serviceValidate?service={}&ticket={}", cas_prefix_url, service, ticket);
    println!("URL: {}", url);
    let (status, body) = http_get(&url).await.map_err(|err| err.to_string())?;
    if status == reqwest::StatusCode::OK {
        if let Some(user) = parse_cas_response(&body) {
            Ok(user.to_owned())
        } else {
            Err(body)
        }
    } else {       
        Err(format!("bad HTTP code {}", status))
    }
}
