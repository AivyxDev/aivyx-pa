with open("win_media.rs", "r") as f:
    c = f.read()
c = c.replace(".and_then(|a: windows::core::IReference<bool>| a.get().ok())", 
              ".and_then(|a: windows::Foundation::IAsyncOperation<windows::Media::Control::GlobalSystemMediaTransportControlsSessionMediaProperties>| a.get().ok())")
with open("win_media.rs", "w") as f:
    f.write(c)
