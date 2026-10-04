use crate::{checked, http, required_env, Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use silicon_iam_client::{models, Client, Credential, EnvironmentKey, IdempotencyKey, Mutation};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Identity {
    pub actor_id: String,
    pub org_id: String,
    pub display_name: String,
    pub admin: bool,
    pub expires_at: i64,
    pub realm: String,
}
// No Debug/Serialize: neither session nor credentials may be accidentally logged.
pub struct IamSession {
    pub identity: Identity,
    pub access_token: String,
    pub refresh_token: Option<String>,
}
#[derive(Clone)]
pub struct Iam {
    client: Client,
    app_id: String,
    testing: bool,
    environment_id: Option<String>,
}
fn unavailable(error: silicon_iam_client::Error) -> Error {
    match error {
        silicon_iam_client::Error::Api(api) if api.code == "idempotency_in_progress" => Error::new(
            "IAM_UNAVAILABLE", "IAM is still processing this operation. Retry with the original request ID.", true),
        silicon_iam_client::Error::Api(api) if api.code == "invalid_grant" => Error::new(
            "IAM_AUTH_FAILED", "IAM rejected the login or refresh grant. Sign in again through IAM.", false),
        silicon_iam_client::Error::Api(api) if api.status < 500 && api.status != 429 => Error::new(
            "IAM_REQUEST_REJECTED", &format!("IAM rejected the operation ({}). Check the application's credentials, scopes and request configuration.", api.code), false),
        silicon_iam_client::Error::Invalid(_) => Error::new("INVALID_IAM_CONFIGURATION", "IAM configuration or request binding is invalid.", false),
        _ => Error::new("IAM_UNAVAILABLE", "IAM did not confirm current identity and authority. Retry with the original operation ID after checking service availability.", true),
    }
}
fn forbidden() -> Error {
    Error::new(
        "IAM_AUTH_FAILED",
        "IAM did not authorize this actor for Ring in the selected organization and realm.",
        false,
    )
}
fn mutation(key: &str) -> Result<Mutation> {
    let digest = format!("{:x}", Sha256::digest(key.as_bytes()));
    IdempotencyKey::parse(digest)
        .map(Mutation::with_key)
        .map_err(unavailable)
}
impl Iam {
    pub fn from_env(test: bool) -> Result<Self> {
        let app_id = std::env::var("RING_IAM_APP_ID").unwrap_or_else(|_| "ring".into());
        let secret = required_env(if test {
            "RING_IAM_TEST_APP_SECRET"
        } else {
            "RING_IAM_APP_SECRET"
        })?;
        let key = if test {
            Some(required_env("RING_IAM_TEST_ENVIRONMENT_KEY")?)
        } else {
            None
        };
        Self::configured(&app_id, &secret, key.as_deref(), None)
    }
    /// Explicit managed-realm credentials never read or change process-wide test secrets.
    pub fn for_testing(environment_id: &str, key: &str, app_secret: &str) -> Result<Self> {
        if uuid::Uuid::parse_str(environment_id)
            .ok()
            .is_none_or(|id| id.is_nil() || id.to_string() != environment_id)
            || app_secret.is_empty()
        {
            return Err(forbidden());
        }
        Self::configured("ring", app_secret, Some(key), Some(environment_id.into()))
    }
    fn configured(
        app_id: &str,
        secret: &str,
        key: Option<&str>,
        environment_id: Option<String>,
    ) -> Result<Self> {
        crate::init_tls();
        let url = std::env::var("RING_IAM_URL")
            .unwrap_or_else(|_| "https://backend.iam.teamofsilicons.com".into());
        let mut client = Client::builder(&url)
            .map_err(unavailable)?
            .telemetry(false)
            .build()
            .map_err(unavailable)?
            .with_credential(Credential::application(app_id, secret));
        if let Some(key) = key {
            client = client.with_environment(EnvironmentKey::new(key).map_err(unavailable)?);
        }
        Ok(Self {
            client,
            app_id: app_id.into(),
            testing: key.is_some(),
            environment_id,
        })
    }
    /// Live IAM validation binds a supplied test secret to its application and exact realm.
    pub async fn testing_environment(&self) -> Result<String> {
        if !self.testing {
            return Err(forbidden());
        }
        let context = self
            .client
            .applications()
            .testing_context()
            .await
            .map_err(unavailable)?;
        let environment = context.environment_id.to_string();
        if context.application.app_id != self.app_id
            || self
                .environment_id
                .as_ref()
                .is_some_and(|expected| expected != &environment)
        {
            return Err(forbidden());
        }
        Ok(environment)
    }
    /// request_id must be durably retained for an exact login retry, including after a lost response.
    pub async fn login(&self, slt: &str, org: &str, request_id: &str) -> Result<IamSession> {
        if slt.is_empty() || request_id.is_empty() {
            return Err(forbidden());
        }
        let tokens = self
            .client
            .oauth()
            .login(&self.app_id, slt, &mutation(request_id)?)
            .await
            .map_err(unavailable)?;
        let identity = self.verify(&tokens.access_token, org).await?;
        Ok(IamSession {
            identity,
            access_token: tokens.access_token,
            refresh_token: Some(tokens.refresh_token),
        })
    }
    pub async fn refresh(
        &self,
        refresh_token: &str,
        org: &str,
        request_id: &str,
    ) -> Result<IamSession> {
        let tokens = self
            .client
            .oauth()
            .refresh(&self.app_id, refresh_token, &mutation(request_id)?)
            .await
            .map_err(unavailable)?;
        let identity = self.verify(&tokens.access_token, org).await?;
        Ok(IamSession {
            identity,
            access_token: tokens.access_token,
            refresh_token: Some(tokens.refresh_token),
        })
    }
    /// Always introspects online; no trust in unverified token payloads or caller identity fields.
    pub async fn verify(&self, access_token: &str, org: &str) -> Result<Identity> {
        let realm = if self.testing {
            Some(self.testing_environment().await?)
        } else {
            None
        };
        let inspected = self
            .client
            .oauth()
            .introspect(
                &models::TokenIntrospectionRequest {
                    token: access_token.into(),
                    token_type_hint: None,
                },
                (!org.is_empty()).then_some(org),
            )
            .await
            .map_err(unavailable)?;
        let value = serde_json::to_value(inspected).map_err(|_| forbidden())?;
        let mut identity = validate_identity(&value, &self.app_id, org, realm.as_deref())?;
        let me = self
            .client
            .with_credential(Credential::bearer(access_token))
            .application_reads()
            .me()
            .await
            .map_err(unavailable)?;
        identity.display_name = me["display_name"]
            .as_str()
            .or_else(|| me["profile"]["display_name"].as_str())
            .unwrap_or(&identity.actor_id)
            .into();
        Ok(identity)
    }
    /// Returns IAM's consent URL; the represented actor must approve the displayed graph in IAM.
    pub async fn request_ting_consent(
        &self,
        access_token: &str,
        org: &str,
        request_id: &str,
    ) -> Result<Value> {
        self.verify(access_token, org).await?;
        let result = self
            .client
            .obo()
            .authorize(
                &models::OboAuthorizationRequest {
                    redirect_uri: None,
                    state: None,
                    subject_token: access_token.into(),
                    org_id: org.into(),
                    endpoints: vec![
                        models::OboAuthorizationEndpoint {
                            audience: "ting".into(),
                            endpoint_id: "tings.send".into(),
                        },
                        models::OboAuthorizationEndpoint {
                            audience: "ting".into(),
                            endpoint_id: "subscriptions.register".into(),
                        },
                    ],
                },
                &mutation(request_id)?,
            )
            .await
            .map_err(unavailable)?;
        serde_json::to_value(result).map_err(|_| forbidden())
    }
    /// The code is only supplied after separate IAM consent. Returned tokens are secrets: persist encrypted, never print.
    pub async fn exchange_ting_consent(
        &self,
        authorization_id: &str,
        code: &str,
        request_id: &str,
    ) -> Result<models::OboTokenResponse> {
        let id = authorization_id.parse().map_err(|_| forbidden())?;
        self.client
            .obo()
            .exchange_code(id, code, &mutation(request_id)?)
            .await
            .map_err(unavailable)
    }
    pub async fn refresh_ting_consent(
        &self,
        refresh_token: &str,
        request_id: &str,
    ) -> Result<models::OboTokenResponse> {
        self.client
            .obo()
            .refresh(refresh_token, &mutation(request_id)?)
            .await
            .map_err(unavailable)
    }
    /// The caller's app token is used for scope-projected directory reads. IAM enforces visibility.
    pub async fn member(&self, access_token: &str, org: &str, actor: &str) -> Result<Value> {
        self.verify(access_token, org).await?;
        if !(actor.starts_with("c:") || actor.starts_with("si:")) || actor.contains(['[', ']', '/'])
        {
            return Err(forbidden());
        }
        let membership = format!("{actor}[{org}]");
        self.client
            .with_credential(Credential::bearer(access_token))
            .application_reads()
            .member(org, &membership)
            .await
            .map_err(unavailable)
    }
}
fn validate_identity(v: &Value, app: &str, org: &str, realm: Option<&str>) -> Result<Identity> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let actor = v["public_id"].as_str().ok_or_else(forbidden)?;
    let kind = v["actor_type"].as_str().ok_or_else(forbidden)?;
    if v["active"] != true
        || !matches!(
            (kind, actor.split(':').next()),
            ("carbon", Some("c")) | ("silicon", Some("si"))
        )
        || (v["client_id"].as_str() != Some(app) && v["audience"].as_str() != Some(app))
        || v["client_id"].as_str().is_some_and(|x| x != app)
        || v["audience"].as_str().is_some_and(|x| x != app)
    {
        return Err(forbidden());
    }
    let expires = v["expires_at"]
        .as_i64()
        .filter(|x| *x > now)
        .ok_or_else(forbidden)?;
    let candidates = v["authorization"]
        .as_object()
        .map(|_| vec![&v["authorization"]])
        .unwrap_or_default();
    let grants: Vec<_> = candidates
        .into_iter()
        .chain(v["authorizations"].as_array().into_iter().flatten())
        .filter(|a| {
            (org.is_empty() || a["org_id"] == org)
                && a["org_id"]
                    .as_str()
                    .is_some_and(|o| !o.is_empty() && !o.contains('|'))
                && a["public_id"] == actor
                && a["audience"] == app
                && a["actor_type"] == kind
                && a["testing_environment_id"].as_str() == realm
                && a["membership_id"].as_str().is_some_and(|m| !m.is_empty())
        })
        .collect();
    let organizations: std::collections::BTreeSet<_> =
        grants.iter().filter_map(|a| a["org_id"].as_str()).collect();
    if org.is_empty() && organizations.len() > 1 {
        let mut error = Error::new(
            "IAM_ORG_REQUIRED",
            "Choose an IAM organization to continue signing in to Ring.",
            false,
        );
        error.details = Some(json!({"organizations": organizations}));
        return Err(error);
    }
    let grant = grants.first().ok_or_else(forbidden)?;
    let org = grant["org_id"].as_str().ok_or_else(forbidden)?;
    Ok(Identity {
        actor_id: actor.into(),
        org_id: org.into(),
        display_name: actor.into(),
        admin: matches!(grant["org_role"].as_str(), Some("owner" | "admin")),
        expires_at: expires,
        realm: realm.unwrap_or("production").into(),
    })
}

#[derive(Clone)]
pub struct Ting {
    http: reqwest::Client,
    origin: String,
    testing: Option<(String, String)>,
}
impl Ting {
    pub fn from_env() -> Result<Self> {
        Self::for_realm(false)
    }
    /// Test requests need Ting's own isolated app credential and the selected IAM environment.
    pub fn for_realm(testing: bool) -> Result<Self> {
        Self::configured(if testing {
            Some((
                required_env("RING_TING_TEST_APP_SECRET")?,
                required_env("RING_IAM_TEST_ENVIRONMENT_KEY")?,
            ))
        } else {
            None
        })
    }
    /// IAM returns the downstream audience's test credential with each OBO endpoint pair.
    pub fn for_grant(context: &Value, expected_key: Option<&str>) -> Result<Self> {
        let testing = match expected_key {
            Some(key) => {
                EnvironmentKey::new(key).map_err(unavailable)?;
                if context["app_id"] != "ting" || context["iam_test_key"] != key {
                    return Err(forbidden());
                }
                let secret = context["app_secret"]
                    .as_str()
                    .filter(|value| !value.is_empty())
                    .ok_or_else(forbidden)?;
                Some((secret.into(), key.into()))
            }
            None if context.is_null() => None,
            None => return Err(forbidden()),
        };
        Self::configured(testing)
    }
    fn configured(testing: Option<(String, String)>) -> Result<Self> {
        let origin = std::env::var("RING_TING_URL")
            .unwrap_or_else(|_| "https://ting.teamofsilicons.com".into());
        let url = reqwest::Url::parse(&origin).map_err(|_| {
            Error::new(
                "INVALID_PROVIDER_URL",
                "Ting URL must be an HTTPS origin.",
                false,
            )
        })?;
        if url.scheme() != "https"
            && !(url.scheme() == "http"
                && matches!(url.host_str(), Some("localhost" | "127.0.0.1")))
        {
            return Err(Error::new(
                "INVALID_PROVIDER_URL",
                "Ting URL must be HTTPS outside localhost.",
                false,
            ));
        }
        Ok(Self {
            http: http()?,
            origin: origin.trim_end_matches('/').into(),
            testing,
        })
    }
    fn request(&self, path: &str, token: &str) -> reqwest::RequestBuilder {
        let mut request = self
            .http
            .post(format!("{}{path}", self.origin))
            .bearer_auth(token);
        if let Some((secret, environment)) = &self.testing {
            request = request
                .header("IAM_TEST_APP_SECRET", secret)
                .header("X-Testing-Environment-Key", environment);
        }
        request
    }
    /// Call only after the actor separately consents to subscriptions.register.
    /// The same actor/app/org tuple is idempotent; repeat it after an uncertain response.
    pub async fn register_subscription(
        &self,
        access_token: &str,
        org: &str,
        recipient: &str,
    ) -> Result<Value> {
        if !recipient.starts_with("si:") || org.is_empty() {
            return Err(forbidden());
        }
        let response = self
            .request("/v1/subscriptions", access_token)
            .json(&json!({"org_id":org,"app_id":"ring","for":recipient}))
            .send()
            .await
            .map_err(|_| Error::network("Ting"))?;
        let value: Value = checked(response, "Ting")
            .await?
            .json()
            .await
            .map_err(|_| Error::network("Ting"))?;
        if value["app_id"] != "ring" || value["for"] != recipient || value["active"] != true {
            return Err(Error::new(
                "TING_SUBSCRIPTION_UNCONFIRMED",
                "Ting did not confirm the selected Ring recipient subscription.",
                true,
            ));
        }
        Ok(value)
    }
    /// Persist these exact bytes in the server's transactional outbox. Do not regenerate event keys on retry.
    pub fn prepare(
        org: &str,
        recipient: &str,
        kind: &str,
        key: &str,
        data: Value,
    ) -> Result<Vec<u8>> {
        if !recipient.starts_with("si:")
            || !kind.starts_with("ring.")
            || key.is_empty()
            || key.len() > 200
        {
            return Err(Error::new(
                "INVALID_NOTIFICATION",
                "Ting requires a silicon recipient, ring event type and stable key.",
                false,
            ));
        }
        serde_json::to_vec(
            &json!({"org_id":org,"for":recipient,"type":kind,"key":key,"data":data,"metadata":{}}),
        )
        .map_err(|_| {
            Error::new(
                "INVALID_NOTIFICATION",
                "Could not serialize notification.",
                false,
            )
        })
    }
    /// Publish an already consented IAM OBO token. Keep body/key identical after an uncertain result.
    /// The server owns encrypted grant storage and refresh; a login token is not notification authority.
    pub async fn publish(&self, access_token: &str, body: &[u8]) -> Result<Value> {
        let value: Value = serde_json::from_slice(body)
            .map_err(|_| Error::new("INVALID_NOTIFICATION", "Corrupt outbox body.", false))?;
        if value["key"].as_str().is_none() || value["org_id"].as_str().is_none() {
            return Err(forbidden());
        }
        let request = self
            .request("/v1/tings", access_token)
            .header("Content-Type", "application/json")
            .body(body.to_vec());
        let response = request.send().await.map_err(|_| Error::network("Ting"))?;
        checked(response, "Ting")
            .await?
            .json()
            .await
            .map_err(|_| Error::network("Ting"))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_rejected_grants_are_auth_failures() {
        for (status, code, expected, retryable) in [
            (400, "invalid_grant", "IAM_AUTH_FAILED", false),
            (401, "invalid_client", "IAM_REQUEST_REJECTED", false),
            (403, "forbidden", "IAM_REQUEST_REJECTED", false),
            (404, "not_found", "IAM_REQUEST_REJECTED", false),
            (409, "idempotency_in_progress", "IAM_UNAVAILABLE", true),
            (500, "internal", "IAM_UNAVAILABLE", true),
            (429, "rate_limited", "IAM_UNAVAILABLE", true),
        ] {
            let error = unavailable(silicon_iam_client::Error::Api(Box::new(
                silicon_iam_client::ApiError {
                    status,
                    code: code.into(),
                    message: "not forwarded".into(),
                    details: None,
                    request_id: None,
                },
            )));
            assert_eq!(error.code, expected);
            assert_eq!(error.retryable, retryable);
        }
    }
    #[test]
    fn identity_checks_actor_audience_membership_realm_and_expiry() {
        let mut v = json!({"active":true,"public_id":"si:alice","actor_type":"silicon","client_id":"ring","expires_at":i64::MAX,"authorization":{"public_id":"si:alice","actor_type":"silicon","audience":"ring","org_id":"tos","membership_id":"si:alice[tos]","testing_environment_id":null,"org_role":"owner"}});
        assert!(validate_identity(&v, "ring", "tos", None).unwrap().admin);
        assert_eq!(
            validate_identity(&v, "ring", "", None).unwrap().org_id,
            "tos"
        );
        assert!(validate_identity(&v, "other", "tos", None).is_err());
        assert!(validate_identity(&v, "ring", "other", None).is_err());
        assert!(validate_identity(&v, "ring", "tos", Some("test")).is_err());
        v["expires_at"] = json!(1);
        assert!(validate_identity(&v, "ring", "tos", None).is_err());
        v["expires_at"] = json!(i64::MAX);
        v["active"] = json!(false);
        assert!(validate_identity(&v, "ring", "tos", None).is_err());
    }
    #[test]
    fn organization_inference_requires_one_matching_iam_membership() {
        let grant = json!({"public_id":"c:alice","actor_type":"carbon","audience":"ring","org_id":"first","membership_id":"c:alice[first]","testing_environment_id":null,"org_role":"member"});
        let mut other = grant.clone();
        other["org_id"] = json!("second");
        other["membership_id"] = json!("c:alice[second]");
        let mut value = json!({"active":true,"public_id":"c:alice","actor_type":"carbon","audience":"ring","expires_at":i64::MAX,"authorizations":[grant,other]});
        let error = validate_identity(&value, "ring", "", None).unwrap_err();
        assert_eq!(error.code, "IAM_ORG_REQUIRED");
        assert_eq!(
            error.details.unwrap()["organizations"],
            json!(["first", "second"])
        );
        assert_eq!(
            validate_identity(&value, "ring", "second", None)
                .unwrap()
                .org_id,
            "second"
        );
        value["authorizations"][1]["audience"] = json!("other-app");
        assert_eq!(
            validate_identity(&value, "ring", "", None).unwrap().org_id,
            "first"
        );
        value["authorizations"][0]["testing_environment_id"] = json!("wrong-realm");
        assert!(validate_identity(&value, "ring", "", None).is_err());
    }
}

pub use silicon_iam_client::webhook::VerifiedWebhook as VerifiedIamWebhook;

/// Authenticate the entire delivery before selecting a realm from its signed metadata.
/// A testing event still requires verify_webhook_environment before any effects.
pub fn verify_webhook(headers: &http::HeaderMap, body: &[u8]) -> Result<VerifiedIamWebhook> {
    use silicon_iam_client::webhook::{WebhookSecret, WebhookSecretKeyring, WebhookVerifier};
    let secret =
        WebhookSecret::new(required_env("RING_IAM_WEBHOOK_SECRET")?).map_err(|_| forbidden())?;
    let version = std::env::var("RING_IAM_WEBHOOK_VERSION")
        .unwrap_or_else(|_| "1".into())
        .parse()
        .map_err(|_| forbidden())?;
    let verifier =
        WebhookVerifier::new(WebhookSecretKeyring::new(version, secret).map_err(|_| forbidden())?);
    verifier.verify(headers, body).map_err(|_| forbidden())
}
pub fn verify_webhook_environment(verified: &VerifiedIamWebhook, key: &str) -> Result<()> {
    verified
        .verify_testing_environment(&EnvironmentKey::new(key).map_err(unavailable)?)
        .map_err(|_| forbidden())
}
