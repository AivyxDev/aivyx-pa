#!/bin/bash
cd crates/aivyx-actions

# Cargo.toml - Add Foundation_Collections
sed -i 's/"Graphics_Imaging",/"Graphics_Imaging",\n    "Foundation_Collections",/' Cargo.toml

# win_system.rs - Use Endpoints::IAudioEndpointVolume and remove unused Interface
sed -i 's/IAudioEndpointVolume, IMMDeviceEnumerator/IMMDeviceEnumerator/' src/desktop/interaction/win_system.rs
sed -i '/use windows::Win32::Media::Audio::/a \        use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;' src/desktop/interaction/win_system.rs
sed -i 's/use windows::core::Interface;//' src/desktop/interaction/win_system.rs
sed -i 's/let _output = tokio::process::Command::new/let _output = tokio::process::Command::new/' src/desktop/interaction/win_system.rs

# win_input.rs - Remove unused flag and fix &MouseButton
sed -i 's/MOUSE_EVENT_FLAGS, MOUSEEVENTF_ABSOLUTE/MOUSEEVENTF_ABSOLUTE/' src/desktop/interaction/win_input.rs
sed -i 's/send_mouse_button_event(from_x, from_y, MouseButton::Left/send_mouse_button_event(from_x, from_y, \&MouseButton::Left/' src/desktop/interaction/win_input.rs
sed -i 's/send_mouse_button_event(to_x, to_y, MouseButton::Left/send_mouse_button_event(to_x, to_y, \&MouseButton::Left/' src/desktop/interaction/win_input.rs

# win_window.rs - Unused imports and HWND struct update and map_err
sed -i 's/SW_SHOW, SWP_NOZORDER/SWP_NOZORDER/' src/desktop/interaction/win_window.rs
sed -i 's/use windows::core::PCWSTR;//' src/desktop/interaction/win_window.rs
sed -i 's/if hwnd.0 == std::ptr::null_mut()/if hwnd.unwrap_or(HWND::default()).0 == std::ptr::null_mut()/' src/desktop/interaction/win_window.rs
sed -i 's/Ok(hwnd)/Ok(hwnd?)/' src/desktop/interaction/win_window.rs

# win_media.rs - Add type annotations (in windows v0.58 some WinRT closures need explicit typing if not inferred)
sed -i 's/\.map(|s| s\.to_string())/.map(|s: windows::core::HSTRING| s.to_string())/g' src/desktop/interaction/win_media.rs
sed -i 's/\.and_then(|a| a\.get()\.ok())/.and_then(|a: windows::core::IReference<bool>| a.get().ok())/g' src/desktop/interaction/win_media.rs
sed -i 's/\.map(|p| p\.Title()/\.map(|p: &windows::Media::Control::GlobalSystemMediaTransportControlsSessionMediaProperties| p.Title()/' src/desktop/interaction/win_media.rs
# Actually, the closures where we do props.Title().map(|s| s.to_string()) can just be inferred if we don't mess up the upstream.
# But compiler said `|s| s.to_string()` requires type annotation because `HSTRING` to string. Wait, if we use `s: windows::core::HSTRING`, it fixes it.

# UnixStream in signal.rs
