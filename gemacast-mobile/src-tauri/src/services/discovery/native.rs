#![cfg(target_os = "android")]

/// Calls the Android Activity's `getTransportType()` method via JNI.
///
/// Returns a pipe-delimited string like `"WIFI:5180|HOTSPOT"` indicating the
/// active network transports and tethering state.
pub fn call_native_transport_check(app: &tauri::AppHandle) -> Result<String, String> {
    use std::sync::mpsc;
    use tauri::Manager;

    let (transport_info_tx, transport_info_rx) = mpsc::channel();

    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "Failed to find main webview window".to_string())?;

    window
        .with_webview(move |webview| {
            let transport_info_tx = transport_info_tx.clone();
            webview.jni_handle().exec(move |env, context, _webview| {
                let result = (|| -> Result<String, String> {
                    let _class = env
                        .get_object_class(context)
                        .map_err(|e| format!("Failed to get Activity class: {}", e))?;

                    let transport_obj = env
                        .call_method(context, "getTransportType", "()Ljava/lang/String;", &[])
                        .map_err(|e| {
                            format!("Failed to call getTransportType on Activity: {}", e)
                        })?;

                    let transport_jstr = transport_obj
                        .l()
                        .map_err(|e| format!("Failed to get transport string object: {}", e))?;

                    let transport: String = env
                        .get_string(&transport_jstr.into())
                        .map_err(|e| format!("Failed to extract string from JNI: {}", e))?
                        .into();

                    Ok(transport)
                })();

                let _ = transport_info_tx.send(result);
            });
        })
        .map_err(|e| format!("WebView JNI execution failed: {}", e))?;

    transport_info_rx
        .recv()
        .map_err(|e| format!("Failed to receive JNI result: {}", e))?
}

fn call_native_string_method(
    app: &tauri::AppHandle,
    method: &'static str,
    argument: Option<String>,
) -> Result<String, String> {
    use std::sync::mpsc;
    use tauri::Manager;

    let (result_tx, result_rx) = mpsc::channel();
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "Failed to find main webview window".to_string())?;
    window
        .with_webview(move |webview| {
            webview.jni_handle().exec(move |env, context, _webview| {
                let result = (|| -> Result<String, String> {
                    let value = if let Some(argument) = argument {
                        let argument = env
                            .new_string(argument)
                            .map_err(|error| format!("Failed to create JNI string: {error}"))?;
                        env.call_method(
                            context,
                            method,
                            "(Ljava/lang/String;)Ljava/lang/String;",
                            &[jni::objects::JValue::from(&argument)],
                        )
                    } else {
                        env.call_method(context, method, "()Ljava/lang/String;", &[])
                    }
                    .map_err(|error| format!("Failed to call {method}: {error}"))?;
                    let value = value
                        .l()
                        .map_err(|error| format!("Failed to read {method} result: {error}"))?;
                    let value: String = env
                        .get_string(&value.into())
                        .map_err(|error| format!("Failed to extract {method} result: {error}"))?
                        .into();
                    if let Some(error) = value.strip_prefix("ERROR:") {
                        Err(error.trim().to_string())
                    } else {
                        Ok(value)
                    }
                })();
                let _ = result_tx.send(result);
            });
        })
        .map_err(|error| format!("WebView JNI execution failed: {error}"))?;
    result_rx
        .recv()
        .map_err(|error| format!("Failed to receive JNI result: {error}"))?
}

pub fn call_native_device_public_key(app: &tauri::AppHandle) -> Result<String, String> {
    call_native_string_method(app, "getDeviceAuthPublicKey", None)
}

pub fn call_native_sign_device_auth(
    app: &tauri::AppHandle,
    transcript_base64: &str,
) -> Result<String, String> {
    call_native_string_method(
        app,
        "signDeviceAuthTranscript",
        Some(transcript_base64.to_string()),
    )
}

pub fn call_native_trusted_pc_fingerprint(
    app: &tauri::AppHandle,
    pc_id: &str,
) -> Result<Option<String>, String> {
    let fingerprint =
        call_native_string_method(app, "getTrustedPcFingerprint", Some(pc_id.to_string()))?;
    Ok((!fingerprint.is_empty()).then_some(fingerprint))
}

pub fn call_native_paired_pc_ids(
    app: &tauri::AppHandle,
) -> Result<Vec<gemacast_core::domain::types::DeviceId>, String> {
    let ids = call_native_string_method(app, "getTrustedPcIds", None)?;
    serde_json::from_str(&ids).map_err(|error| format!("invalid paired PC ID list: {error}"))
}

pub fn call_native_confirm_pc_identity(
    app: &tauri::AppHandle,
    pc_id: &str,
    pc_name: &str,
    fingerprint: &str,
    pairing_code: &str,
    requires_approval: bool,
) -> Result<bool, String> {
    const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(150);
    const CONFIRMATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(65);

    let payload = serde_json::json!({
        "pcId": pc_id,
        "pcName": pc_name,
        "fingerprint": fingerprint,
        "pairingCode": pairing_code,
        "requiresApproval": requires_approval,
    })
    .to_string();
    let initial = call_native_string_method(app, "confirmPcIdentity", Some(payload.clone()))?;
    match initial.as_str() {
        "APPROVED" | "TRUSTED" => Ok(true),
        "REJECTED" => Ok(false),
        "PENDING" => {
            let started = std::time::Instant::now();
            loop {
                std::thread::sleep(POLL_INTERVAL);
                let result = match call_native_string_method(
                    app,
                    "pollPcIdentityConfirmation",
                    Some(payload.clone()),
                ) {
                    Ok(result) => result,
                    Err(error) => {
                        let _ = call_native_string_method(
                            app,
                            "cancelPcIdentityConfirmation",
                            Some(payload),
                        );
                        return Err(error);
                    }
                };
                match result.as_str() {
                    "APPROVED" => return Ok(true),
                    "REJECTED" => {
                        let _ = call_native_string_method(
                            app,
                            "cancelPcIdentityConfirmation",
                            Some(payload),
                        );
                        return Ok(false);
                    }
                    "PENDING" if started.elapsed() < CONFIRMATION_TIMEOUT => {}
                    "PENDING" => {
                        let _ = call_native_string_method(
                            app,
                            "cancelPcIdentityConfirmation",
                            Some(payload),
                        );
                        return Err("PC identity confirmation timed out on the phone".to_string());
                    }
                    other => {
                        let _ = call_native_string_method(
                            app,
                            "cancelPcIdentityConfirmation",
                            Some(payload),
                        );
                        return Err(format!(
                            "unexpected PC identity confirmation result: {other}"
                        ));
                    }
                }
            }
        }
        other => Err(format!(
            "unexpected PC identity confirmation result: {other}"
        )),
    }
}

pub fn call_native_remember_pc_identity(
    app: &tauri::AppHandle,
    pc_id: &str,
    fingerprint: &str,
) -> Result<(), String> {
    let payload = serde_json::json!({
        "pcId": pc_id,
        "fingerprint": fingerprint,
    })
    .to_string();
    let result = call_native_string_method(app, "rememberPcIdentity", Some(payload))?;
    if result == "OK" {
        Ok(())
    } else {
        Err(format!(
            "unexpected PC identity persistence result: {result}"
        ))
    }
}

pub fn call_native_forget_pc_identity(app: &tauri::AppHandle, pc_id: &str) -> Result<(), String> {
    let result = call_native_string_method(app, "forgetPcIdentity", Some(pc_id.to_string()))?;
    if result == "OK" {
        Ok(())
    } else {
        Err(format!("unexpected PC identity removal result: {result}"))
    }
}

/// Ask Android to finish the activity and drop its task record, on the way out.
///
/// Bounded wait rather than [`call_native_string_method`]'s blocking `recv()`: this
/// runs on the exit path, and a webview or main thread that never answers must not
/// be able to keep the process alive. The Kotlin side does not kill the process —
/// the caller's `AppHandle::exit` does — so timing out here only costs the task
/// record cleanup, not the exit itself.
pub fn call_native_finish_and_remove_task(app: &tauri::AppHandle) -> Result<(), String> {
    use std::sync::mpsc;
    use tauri::Manager;

    const EXIT_JNI_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1500);

    let (result_tx, result_rx) = mpsc::channel();
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "Failed to find main webview window".to_string())?;
    window
        .with_webview(move |webview| {
            webview.jni_handle().exec(move |env, context, _webview| {
                let result = env
                    .call_method(
                        context,
                        "finishAndRemoveAppTask",
                        "()Ljava/lang/String;",
                        &[],
                    )
                    .map_err(|error| format!("Failed to call finishAndRemoveAppTask: {error}"))
                    .map(|_| ());
                let _ = result_tx.send(result);
            });
        })
        .map_err(|error| format!("WebView JNI execution failed: {error}"))?;
    result_rx
        .recv_timeout(EXIT_JNI_TIMEOUT)
        .map_err(|error| format!("Failed to receive JNI result: {error}"))?
}

/// Calls the Android Activity's `syncServiceState()` method via JNI.
pub fn call_native_sync_service(
    app: &tauri::AppHandle,
    action: &str,
    is_exclusive: bool,
) -> Result<(), String> {
    use tauri::Manager;

    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "Failed to find main webview window".to_string())?;

    let action_str = action.to_string();

    window
        .with_webview(move |webview| {
            webview.jni_handle().exec(move |env, context, _webview| {
                let action_jstr = env.new_string(&action_str).unwrap();
                let _ = env.call_method(
                    context,
                    "syncServiceState",
                    "(Ljava/lang/String;Z)V",
                    &[
                        jni::objects::JValue::from(&action_jstr),
                        jni::objects::JValue::from(is_exclusive),
                    ],
                );
            });
        })
        .map_err(|e| format!("WebView JNI execution failed: {}", e))?;

    Ok(())
}
