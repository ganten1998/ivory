fn main() {
    #[cfg(windows)]
    {
        let mut res = winres::WindowsResource::new();
        // Icon is generated into assets/ by scripts; tolerate absence in dev.
        if std::path::Path::new("../assets/ivory.ico").exists() {
            res.set_icon("../assets/ivory.ico");
        }
        res.set("ProductName", "Ivory");
        res.set("FileDescription", "Ivory - MIDI Keyboard Monitor");
        let _ = res.compile();
    }
}
