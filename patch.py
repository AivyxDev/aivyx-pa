import os

def fix_system():
    p = 'crates/aivyx-actions/src/desktop/interaction/win_system.rs'
    with open(p, 'r') as f: c = f.read()
    c = c.replace('IAudioEndpointVolume, IMMDeviceEnumerator, MMDeviceEnumerator, eConsole, eRender', 
                  'IMMDeviceEnumerator, MMDeviceEnumerator, eConsole, eRender')
    c = c.replace('use windows::Win32::Media::Audio::{', 
                  'use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;\n        use windows::Win32::Media::Audio::{')
    c = c.replace('use windows::core::Interface;\n', '')
    with open(p, 'w') as f: f.write(c)

def fix_input():
    p = 'crates/aivyx-actions/src/desktop/interaction/win_input.rs'
    with open(p, 'r') as f: c = f.read()
    c = c.replace('MOUSE_EVENT_FLAGS, MOUSEEVENTF_ABSOLUTE', 'MOUSEEVENTF_ABSOLUTE')
    c = c.replace('send_mouse_button_event(from_x, from_y, MouseButton::Left, true)',
                  'send_mouse_button_event(from_x, from_y, &MouseButton::Left, true)')
    c = c.replace('send_mouse_button_event(to_x, to_y, MouseButton::Left, false)',
                  'send_mouse_button_event(to_x, to_y, &MouseButton::Left, false)')
    with open(p, 'w') as f: f.write(c)

def fix_window():
    p = 'crates/aivyx-actions/src/desktop/interaction/win_window.rs'
    with open(p, 'r') as f: c = f.read()
    c = c.replace('SW_SHOW, SWP_NOZORDER', 'SWP_NOZORDER')
    c = c.replace('use windows::core::PCWSTR;\n', '')
    # fix hwnd.0 == null
    c = c.replace('if hwnd.0 == std::ptr::null_mut() {', 'if hwnd.unwrap_or_default().0 == std::ptr::null_mut() {')
    c = c.replace('FindWindowW(PCWSTR::null(), PCWSTR(wide.as_ptr()))', 'FindWindowW(windows::core::PCWSTR::null(), windows::core::PCWSTR(wide.as_ptr()))')
    with open(p, 'w') as f: f.write(c)

def fix_screenshot():
    p = 'crates/aivyx-actions/src/desktop/interaction/win_screenshot.rs'
    with open(p, 'r') as f: c = f.read()
    c = c.replace('if hwnd.0 == std::ptr::null_mut() {', 'if hwnd.unwrap_or_default().0 == std::ptr::null_mut() {')
    c = c.replace('capture_hwnd(hwnd).await', 'capture_hwnd(hwnd.unwrap_or_default()).await')
    c = c.replace('use std::fmt::Write as FmtWrite;\n', '')
    with open(p, 'w') as f: f.write(c)

def fix_media():
    p = 'crates/aivyx-actions/src/desktop/interaction/win_media.rs'
    with open(p, 'r') as f: c = f.read()
    c = c.replace('let info = session', 'let info: Option<windows::Media::Control::GlobalSystemMediaTransportControlsSessionMediaProperties> = session')
    c = c.replace('.map(|s| s.to_string())', '.map(|s| s.to_string_lossy())') # HSTRING has to_string_lossy or to_string internally, but let's just use to_string() and explicit type if needed. Actually it's .map(|s| s.to_string()) type inference fails.
    # To fix inference, we can do `.map(|s: windows::core::HSTRING| s.to_string())`
    c = c.replace('.map(|s| s.to_string())', '.map(|s: windows::core::HSTRING| s.to_string())')
    with open(p, 'w') as f: f.write(c)

fix_system()
fix_input()
fix_window()
fix_screenshot()
fix_media()
