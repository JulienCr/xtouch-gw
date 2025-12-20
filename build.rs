fn main() {
    // Only compile resources on Windows
    #[cfg(windows)]
    {
        embed_resource::compile("resources.rc", embed_resource::NONE);
    }
}
