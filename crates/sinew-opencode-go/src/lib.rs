mod auth;
mod client;
mod model_info;
mod stream;
mod wire;

pub use auth::{
    delete_default_auth, load_default_api_key, load_default_auth_status, save_default_api_key,
    touch_default_auth_validation, Credential, OpenCodeGoAuthStatus,
};
pub use client::{validate_api_key, OpenCodeGoConfig, OpenCodeGoProvider};
pub use model_info::{capabilities, MODELS, MODEL_ID, MODEL_MAX_OUTPUT, MODEL_WINDOW, PROVIDER_ID};
