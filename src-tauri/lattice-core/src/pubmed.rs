use crate::models::LiteratureHit;
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use serde::Deserialize;
use std::io::Read;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const ENDPOINT: &str = "https://eutils.ncbi.nlm.nih.gov/entrez/eutils";
const PER_PAGE: usize = 25;
const MAX_RESULTS: usize = 10_000;
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const REQUEST_INTERVAL: Duration = Duration::from_millis(334);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

static NEXT_REQUEST: Mutex<Option<Instant>> = Mutex::new(None);

#[derive(Debug)]
pub struct SearchPage {
    pub hits: Vec<LiteratureHit>,
    pub has_more: bool,
}

#[derive(Deserialize)]
struct SearchResponse {
    esearchresult: SearchResult,
}

#[derive(Deserialize)]
struct SearchResult {
    count: String,
    #[serde(default, rename = "idlist")]
    ids: Vec<String>,
}

#[derive(Default)]
struct Article {
    external_id: String,
    title: String,
    authors: Vec<String>,
    author: Option<Author>,
    abstract_text: String,
    year: Option<u32>,
    doi: Option<String>,
    reading_doi: bool,
    journal: String,
    journal_abbreviation: String,
    article_id_type: Option<String>,
}

#[derive(Default)]
struct Author {
    given: String,
    family: String,
    collective: String,
}

pub fn search_works(query: &str, precise: bool, page: u32) -> Result<SearchPage, String> {
    let email = crate::literature_credentials::ncbi_email()?
        .ok_or_else(|| "NCBI contact email is not configured.".to_string())?;
    search_works_at(query, precise, page, &email, ENDPOINT)
}

fn search_works_at(
    query: &str, precise: bool, page: u32, email: &str, endpoint: &str,
) -> Result<SearchPage, String> {
    let start = (page as usize).saturating_mul(PER_PAGE);
    if start >= MAX_RESULTS {
        return Ok(SearchPage { hits: Vec::new(), has_more: false });
    }
    let client = crate::literature_service::client(REQUEST_TIMEOUT, Some("Lattice"))
        .map_err(|_| "Could not create the PubMed client.".to_string())?;
    let term = if precise {
        format!("({})[Title/Abstract]", query.trim())
    } else {
        query.trim().to_string()
    };
    let mut search_url =
        reqwest::Url::parse(&format!("{}/esearch.fcgi", endpoint.trim_end_matches('/')))
            .map_err(|_| "Invalid PubMed endpoint.".to_string())?;
    search_url
        .query_pairs_mut()
        .append_pair("db", "pubmed")
        .append_pair("term", &term)
        .append_pair("retmode", "json")
        .append_pair("retmax", &PER_PAGE.to_string())
        .append_pair("retstart", &start.to_string())
        .append_pair("sort", "relevance")
        .append_pair("tool", "Lattice")
        .append_pair("email", email);
    wait_for_request_slot();
    let response =
        client.get(search_url).send().map_err(|_| "PubMed search request failed.".to_string())?;
    if !response.status().is_success() {
        return Err(format!("PubMed search returned HTTP {}.", response.status().as_u16()));
    }
    let search_bytes = read_capped(response)?;
    let search: SearchResponse = serde_json::from_slice(&search_bytes)
        .map_err(|_| "PubMed returned an invalid search response.".to_string())?;
    let total = search
        .esearchresult
        .count
        .parse::<usize>()
        .map_err(|_| "PubMed returned an invalid result count.".to_string())?;
    let ids: Vec<_> = search
        .esearchresult
        .ids
        .into_iter()
        .filter(|id| !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()))
        .collect();
    if ids.is_empty() {
        return Ok(SearchPage { hits: Vec::new(), has_more: false });
    }

    let mut fetch_url =
        reqwest::Url::parse(&format!("{}/efetch.fcgi", endpoint.trim_end_matches('/')))
            .map_err(|_| "Invalid PubMed endpoint.".to_string())?;
    fetch_url
        .query_pairs_mut()
        .append_pair("db", "pubmed")
        .append_pair("id", &ids.join(","))
        .append_pair("retmode", "xml")
        .append_pair("tool", "Lattice")
        .append_pair("email", email);
    wait_for_request_slot();
    let response =
        client.get(fetch_url).send().map_err(|_| "PubMed record request failed.".to_string())?;
    if !response.status().is_success() {
        return Err(format!("PubMed record request returned HTTP {}.", response.status().as_u16()));
    }
    let xml = read_capped(response)?;
    let hits = parse_articles(&xml)?;
    let has_more = start.saturating_add(ids.len()) < total.min(MAX_RESULTS);
    Ok(SearchPage { hits, has_more })
}

fn wait_for_request_slot() {
    let delay = {
        let mut next = NEXT_REQUEST.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let now = Instant::now();
        let scheduled = next.filter(|scheduled| *scheduled > now).unwrap_or(now);
        *next = Some(scheduled + REQUEST_INTERVAL);
        scheduled.saturating_duration_since(now)
    };
    std::thread::sleep(delay);
}

fn read_capped(response: reqwest::blocking::Response) -> Result<Vec<u8>, String> {
    if response.content_length().is_some_and(|length| length > MAX_RESPONSE_BYTES as u64) {
        return Err("PubMed returned an oversized record response.".to_string());
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_RESPONSE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Could not read the PubMed record response.".to_string())?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err("PubMed returned an oversized record response.".to_string());
    }
    Ok(bytes)
}

fn parse_articles(xml: &[u8]) -> Result<Vec<LiteratureHit>, String> {
    let mut reader = Reader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut tags = Vec::new();
    let mut article: Option<Article> = None;
    let mut hits = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(start)) => {
                let tag = String::from_utf8_lossy(start.local_name().as_ref()).into_owned();
                if tag == "PubmedArticle" {
                    article = Some(Article::default());
                }
                if tag == "Author" && article.is_some() {
                    article.as_mut().unwrap().author = Some(Author::default());
                }
                if tag == "ArticleId" {
                    if let Some(article) = article.as_mut() {
                        article.article_id_type = start
                            .attributes()
                            .with_checks(false)
                            .filter_map(Result::ok)
                            .find(|attribute| attribute.key.as_ref() == b"IdType")
                            .map(|attribute| {
                                String::from_utf8_lossy(attribute.value.as_ref())
                                    .to_ascii_lowercase()
                            });
                        article.reading_doi = article.article_id_type.as_deref() == Some("doi");
                    }
                }
                if tag == "AbstractText" {
                    if let Some(article) = article.as_mut() {
                        if !article.abstract_text.is_empty() {
                            article.abstract_text.push(' ');
                        }
                    }
                }
                tags.push(tag);
            }
            Ok(Event::Empty(start)) => {
                if start.local_name().as_ref() == b"ArticleId" {
                    if let Some(article) = article.as_mut() {
                        article.reading_doi = false;
                        article.article_id_type = None;
                    }
                }
            }
            Ok(Event::Text(text)) => {
                let text = text
                    .xml10_content()
                    .map_err(|_| "PubMed returned invalid XML text.".to_string())?;
                append_text(article.as_mut(), &tags, &text);
            }
            Ok(Event::CData(text)) => {
                let text = text
                    .xml10_content()
                    .map_err(|_| "PubMed returned invalid XML text.".to_string())?;
                append_text(article.as_mut(), &tags, &text);
            }
            Ok(Event::GeneralRef(reference)) => {
                let decoded = reference
                    .decode()
                    .map_err(|_| "PubMed returned invalid XML text.".to_string())?;
                let text = reference
                    .resolve_char_ref()
                    .ok()
                    .flatten()
                    .map(|ch| ch.to_string())
                    .or_else(|| {
                        quick_xml::escape::resolve_predefined_entity(&decoded).map(str::to_string)
                    })
                    .unwrap_or_default();
                append_text(article.as_mut(), &tags, &text);
            }
            Ok(Event::End(end)) => {
                let tag = end.local_name();
                if tag.as_ref() == b"Author" {
                    if let Some(article) = article.as_mut() {
                        if let Some(author) = article.author.take() {
                            let name = if !author.collective.trim().is_empty() {
                                author.collective
                            } else {
                                format!("{} {}", author.given.trim(), author.family.trim())
                                    .trim()
                                    .to_string()
                            };
                            if !name.is_empty() {
                                article.authors.push(name);
                            }
                        }
                    }
                }
                if tag.as_ref() == b"ArticleId" {
                    if let Some(article) = article.as_mut() {
                        article.reading_doi = false;
                        article.article_id_type = None;
                    }
                }
                if tag.as_ref() == b"PubmedArticle" {
                    if let Some(article) = article.take() {
                        if let Some(hit) = article.into_hit() {
                            hits.push(hit);
                        }
                    }
                }
                tags.pop();
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(_) => return Err("PubMed returned invalid XML.".to_string()),
        }
        buffer.clear();
    }
    Ok(hits)
}

fn append_text(article: Option<&mut Article>, tags: &[String], text: &str) {
    let Some(article) = article else { return };
    let tag = tags
        .iter()
        .rev()
        .find(|tag| {
            matches!(
                tag.as_str(),
                "ArticleTitle"
                    | "AbstractText"
                    | "LastName"
                    | "ForeName"
                    | "Initials"
                    | "CollectiveName"
                    | "Year"
                    | "MedlineDate"
                    | "ISOAbbreviation"
                    | "Title"
                    | "ArticleId"
                    | "PMID"
            )
        })
        .map(String::as_str);
    match tag {
        Some("ArticleTitle") => article.title.push_str(text),
        Some("AbstractText") => article.abstract_text.push_str(text),
        Some("LastName") if article.author.is_some() => {
            article.author.as_mut().unwrap().family.push_str(text)
        }
        Some("ForeName" | "Initials") if article.author.is_some() => {
            article.author.as_mut().unwrap().given.push_str(text)
        }
        Some("CollectiveName") if article.author.is_some() => {
            article.author.as_mut().unwrap().collective.push_str(text)
        }
        Some("Year" | "MedlineDate")
            if tags.iter().any(|tag| tag == "PubDate" || tag == "ArticleDate") =>
        {
            if article.year.is_none() {
                article.year = first_year(text);
            }
        }
        Some("ISOAbbreviation") if tags.iter().any(|tag| tag == "Journal") => {
            article.journal_abbreviation.push_str(text)
        }
        Some("Title") if tags.iter().any(|tag| tag == "Journal") => article.journal.push_str(text),
        Some("ArticleId") if article.reading_doi => {
            article.doi.get_or_insert_with(String::new).push_str(text)
        }
        Some("ArticleId")
            if article.external_id.is_empty()
                && article.article_id_type.as_deref() == Some("pubmed") =>
        {
            article.external_id.push_str(text)
        }
        Some("PMID") if tags.iter().any(|tag| tag == "MedlineCitation") => {
            article.external_id.push_str(text)
        }
        _ => {}
    }
}

fn first_year(text: &str) -> Option<u32> {
    let bytes = text.as_bytes();
    bytes.windows(4).find_map(|window| {
        window
            .iter()
            .all(u8::is_ascii_digit)
            .then(|| std::str::from_utf8(window).ok()?.parse().ok())
            .flatten()
    })
}

impl Article {
    fn into_hit(self) -> Option<LiteratureHit> {
        let title = crate::util::collapse_whitespace(&self.title);
        if title.is_empty() {
            return None;
        }
        let doi = self.doi.and_then(|doi| crate::project::normalize_doi(&doi));
        let abstract_text = crate::util::collapse_whitespace(&self.abstract_text);
        Some(LiteratureHit {
            source: "pubmed".into(),
            arxiv_id: None,
            title,
            year: self.year,
            authors: self.authors.into_iter().take(20).collect(),
            cited_by_count: None,
            votes: None,
            snippet: None,
            r#abstract: (!abstract_text.is_empty()).then_some(abstract_text),
            doi,
            landing_url: None,
            venue: nonempty(&self.journal_abbreviation).or_else(|| nonempty(&self.journal)),
            external_id: nonempty(&self.external_id),
        })
    }
}

fn nonempty(value: &str) -> Option<String> {
    let value = crate::util::collapse_whitespace(value);
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiny_http::{Response, Server};

    #[test]
    fn parses_medline_xml_into_search_hits() {
        let xml = br#"<PubmedArticleSet><PubmedArticle><MedlineCitation><Article>
          <ArticleTitle>Effects of <i>RNA</i> &amp; protein</ArticleTitle>
          <Abstract><AbstractText>First &#x3B2; result.</AbstractText><AbstractText>Second <i>finding</i>.</AbstractText></Abstract>
          <AuthorList><Author><LastName>Smith</LastName><ForeName>Jane</ForeName></Author><Author><CollectiveName>Research Group</CollectiveName></Author></AuthorList>
          <Journal><Title>Journal of Testing</Title><ISOAbbreviation>J Test</ISOAbbreviation><JournalIssue><PubDate><MedlineDate>2024 Jan-Feb</MedlineDate></PubDate></JournalIssue></Journal>
        </Article></MedlineCitation><PubmedData><ArticleIdList><ArticleId IdType="pubmed">123</ArticleId><ArticleId IdType="doi">10.1234/ABC</ArticleId></ArticleIdList></PubmedData></PubmedArticle></PubmedArticleSet>"#;
        let hits = parse_articles(xml).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].source, "pubmed");
        assert_eq!(hits[0].title, "Effects of RNA & protein");
        assert_eq!(hits[0].authors, ["Jane Smith", "Research Group"]);
        assert_eq!(hits[0].year, Some(2024));
        assert_eq!(hits[0].doi.as_deref(), Some("10.1234/abc"));
        assert_eq!(hits[0].external_id.as_deref(), Some("123"));
        assert_eq!(hits[0].venue.as_deref(), Some("J Test"));
        assert_eq!(hits[0].r#abstract.as_deref(), Some("First β result. Second finding."));
    }

    #[test]
    fn searches_a_page_then_fetches_its_records() {
        let server = Server::http("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/entrez/eutils", server.server_addr());
        let responder = std::thread::spawn(move || {
            let search = server.recv().unwrap();
            let url = reqwest::Url::parse(&format!("http://localhost{}", search.url())).unwrap();
            let params: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
            assert_eq!(params["db"], "pubmed");
            assert_eq!(params["term"], "(enzyme)[Title/Abstract]");
            assert_eq!(params["retstart"], "25");
            assert_eq!(params["email"], "maintainer@example.org");
            search
                .respond(Response::from_string(
                    r#"{"esearchresult":{"count":"30","idlist":["12345"]}}"#,
                ))
                .unwrap();
            let fetch = server.recv().unwrap();
            assert!(fetch.url().contains("efetch.fcgi"));
            fetch.respond(Response::from_data(br#"<PubmedArticleSet><PubmedArticle><MedlineCitation><Article><ArticleTitle>A study</ArticleTitle></Article></MedlineCitation><PubmedData><ArticleIdList><ArticleId IdType="doi">10.1234/study</ArticleId></ArticleIdList></PubmedData></PubmedArticle></PubmedArticleSet>"#.to_vec())).unwrap();
        });
        let page = search_works_at("enzyme", true, 1, "maintainer@example.org", &endpoint).unwrap();
        responder.join().unwrap();
        assert!(page.has_more);
        assert_eq!(page.hits.len(), 1);
        assert_eq!(page.hits[0].doi.as_deref(), Some("10.1234/study"));
    }
}
