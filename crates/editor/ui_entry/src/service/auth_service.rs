use gpui::RenderImage;
use std::sync::Arc;

pub struct AuthService;

impl AuthService {
    pub fn profile() -> Option<engine_state::AuthProfile> {
        engine_state::EngineContext::global().and_then(|ec| ec.auth_profile())
    }

    pub fn begin_github_sign_in(
        on_code: Box<dyn Fn(String, String) + Send + Sync>,
        on_message: Box<dyn Fn(String) + Send + Sync>,
        on_complete: Box<dyn Fn(Option<engine_state::AuthProfile>) + Send + Sync + 'static>,
    ) {
        let Some(client_id) = pulsar_auth::github_client_id_from_env() else {
            on_message("Set PULSAR_GITHUB_CLIENT_ID to enable GitHub sign-in.".to_string());
            return;
        };

        on_message("Starting GitHub sign-in\u{2026}".to_string());

        std::thread::spawn(move || match pulsar_auth::start_device_flow(&client_id) {
            Ok(flow) => {
                on_code(flow.user_code.clone(), flow.verification_uri.clone());
                match pulsar_auth::wait_for_device_flow_token(&client_id, &flow) {
                    Ok(token) => match pulsar_auth::fetch_profile(&token) {
                        Ok(profile) => {
                            let _ = pulsar_auth::store_access_token(&token);
                            let _ = pulsar_auth::save_cached_profile(&profile);
                            if let Some(ec) = engine_state::EngineContext::global() {
                                ec.set_auth_profile(profile.clone());
                            }
                            on_complete(Some(profile));
                        }
                        Err(e) => on_complete(None),
                    },
                    Err(_) => on_complete(None),
                }
            }
            Err(_) => on_complete(None),
        });
    }

    pub fn load_avatar(url: &str) -> Option<Arc<RenderImage>> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .user_agent("Pulsar-Native/1.0")
            .build()
            .ok()?;
        let response = client.get(url).send().ok()?;
        let bytes = response.bytes().ok()?;
        let rgba = image::load_from_memory(&bytes).ok()?.into_rgba8();
        let frame = image::Frame::new(rgba);
        Some(Arc::new(RenderImage::new(smallvec::smallvec![frame])))
    }
}
