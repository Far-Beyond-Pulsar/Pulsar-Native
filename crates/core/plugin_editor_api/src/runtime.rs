pub trait PluginRuntime {
    fn start(&mut self) -> &mut dyn PluginRuntime;
    fn stop(&mut self);
}