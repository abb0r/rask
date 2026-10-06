fn main() {
    slint_build::compile("ui/app.slint").expect("compile Slint UI");
    #[cfg(target_os = "windows")]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("packaging/windows/rask.ico");
        res.compile().expect("embed Windows icon");
    }
}
