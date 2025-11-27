//! NetworkManager D-Bus integration for WiFi connections
//!
//! This module provides WiFi connection functionality via NetworkManager's D-Bus API,
//! which works in both native and flatpak environments (with appropriate D-Bus permissions).

use std::collections::HashMap;
use tracing::{error, info};
use zbus::zvariant::{ObjectPath, Value};

/// Connect to a WiFi network using NetworkManager's D-Bus API
///
/// # Arguments
/// * `ssid` - The network SSID
/// * `password` - Optional password (None for open networks)
/// * `security` - Security type (e.g., "WPA", "WPA2", "WPA3", "WEP", "OPEN")
/// * `hidden` - Whether the network is hidden
pub async fn connect_wifi(
    ssid: String,
    password: Option<String>,
    security: String,
    hidden: bool,
) -> Result<(), String> {
    info!(
        ssid = %ssid,
        security = %security,
        hidden,
        has_password = password.is_some(),
        "Connecting to WiFi via NetworkManager D-Bus"
    );

    // Connect to the system bus
    let connection = zbus::Connection::system()
        .await
        .map_err(|e| format!("Failed to connect to system D-Bus: {}", e))?;

    // Build connection settings
    let settings = build_connection_settings(&ssid, password.as_deref(), &security, hidden);

    // Find a suitable WiFi device
    let device_path = find_wifi_device(&connection).await?;

    // Use AddAndActivateConnection to add and connect in one step
    let nm_proxy = zbus::Proxy::new(
        &connection,
        "org.freedesktop.NetworkManager",
        "/org/freedesktop/NetworkManager",
        "org.freedesktop.NetworkManager",
    )
    .await
    .map_err(|e| format!("Failed to create NetworkManager proxy: {}", e))?;

    // Call AddAndActivateConnection(settings, device_path, specific_object)
    // specific_object is "/" for no specific access point
    let no_specific_object = ObjectPath::try_from("/").unwrap();
    let result: Result<
        (
            zbus::zvariant::OwnedObjectPath,
            zbus::zvariant::OwnedObjectPath,
        ),
        _,
    > = nm_proxy
        .call(
            "AddAndActivateConnection",
            &(settings, &device_path, &no_specific_object),
        )
        .await;

    match result {
        Ok((connection_path, active_connection_path)) => {
            info!(
                connection = %connection_path,
                active = %active_connection_path,
                ssid = %ssid,
                "WiFi connection activated successfully"
            );
            Ok(())
        }
        Err(e) => {
            error!(ssid = %ssid, error = %e, "Failed to activate WiFi connection");
            Err(format!("Failed to connect to WiFi: {}", e))
        }
    }
}

/// Build the connection settings dictionary for NetworkManager
fn build_connection_settings<'a>(
    ssid: &'a str,
    password: Option<&'a str>,
    security: &str,
    hidden: bool,
) -> HashMap<&'a str, HashMap<&'a str, Value<'a>>> {
    let mut settings: HashMap<&str, HashMap<&str, Value>> = HashMap::new();

    // Connection settings
    let mut connection: HashMap<&str, Value> = HashMap::new();
    connection.insert("type", Value::new("802-11-wireless"));
    connection.insert("id", Value::new(ssid));
    // Generate a UUID for the connection
    let uuid_str = uuid::Uuid::new_v4().to_string();
    connection.insert("uuid", Value::new(uuid_str));
    settings.insert("connection", connection);

    // Wireless settings
    let mut wireless: HashMap<&str, Value> = HashMap::new();
    // SSID must be sent as bytes
    wireless.insert("ssid", Value::new(ssid.as_bytes().to_vec()));
    wireless.insert("mode", Value::new("infrastructure"));
    if hidden {
        wireless.insert("hidden", Value::new(true));
    }
    settings.insert("802-11-wireless", wireless);

    // Security settings (if not open network)
    let key_mgmt = match security.to_uppercase().as_str() {
        "OPEN" | "NOPASS" => None,
        "WEP" => Some("none"), // WEP uses "none" for key-mgmt but sets wep keys
        "WPA" | "WPA2" | "WPA/WPA2" => Some("wpa-psk"),
        "WPA3" | "SAE" => Some("sae"),
        "ENTERPRISE" | "WPA-EAP" => Some("wpa-eap"),
        _ => Some("wpa-psk"), // Default to WPA-PSK
    };

    if let Some(km) = key_mgmt {
        let mut wireless_security: HashMap<&str, Value> = HashMap::new();
        wireless_security.insert("key-mgmt", Value::new(km));
        wireless_security.insert("auth-alg", Value::new("open"));

        if let Some(pwd) = password {
            if km == "none" {
                // WEP key
                wireless_security.insert("wep-key0", Value::new(pwd));
                wireless_security.insert("wep-key-type", Value::new(1u32)); // 1 = passphrase
            } else {
                // WPA/WPA2/WPA3 PSK
                wireless_security.insert("psk", Value::new(pwd));
            }
        }

        settings.insert("802-11-wireless-security", wireless_security);

        // Tell wireless settings about security
        if let Some(w) = settings.get_mut("802-11-wireless") {
            w.insert("security", Value::new("802-11-wireless-security"));
        }
    }

    // IPv4 settings - auto
    let mut ipv4: HashMap<&str, Value> = HashMap::new();
    ipv4.insert("method", Value::new("auto"));
    settings.insert("ipv4", ipv4);

    // IPv6 settings - auto
    let mut ipv6: HashMap<&str, Value> = HashMap::new();
    ipv6.insert("method", Value::new("auto"));
    settings.insert("ipv6", ipv6);

    settings
}

/// Find the first available WiFi device
async fn find_wifi_device(
    connection: &zbus::Connection,
) -> Result<zbus::zvariant::OwnedObjectPath, String> {
    let nm_proxy = zbus::Proxy::new(
        connection,
        "org.freedesktop.NetworkManager",
        "/org/freedesktop/NetworkManager",
        "org.freedesktop.NetworkManager",
    )
    .await
    .map_err(|e| format!("Failed to create NetworkManager proxy: {}", e))?;

    // Get all devices
    let devices: Vec<zbus::zvariant::OwnedObjectPath> = nm_proxy
        .call("GetDevices", &())
        .await
        .map_err(|e| format!("Failed to get devices: {}", e))?;

    // Find a WiFi device (DeviceType 2 = WiFi)
    for device_path in devices {
        let device_proxy = zbus::Proxy::new(
            connection,
            "org.freedesktop.NetworkManager",
            device_path.as_str(),
            "org.freedesktop.NetworkManager.Device",
        )
        .await
        .map_err(|e| format!("Failed to create device proxy: {}", e))?;

        let device_type: u32 = device_proxy.get_property("DeviceType").await.unwrap_or(0);

        // DeviceType 2 = NM_DEVICE_TYPE_WIFI
        if device_type == 2 {
            info!(device = %device_path, "Found WiFi device");
            return Ok(device_path);
        }
    }

    Err("No WiFi device found".to_string())
}
