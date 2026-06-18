use scarlet_ui::prelude::*;
use scarlet_ui::vstack;

#[derive(Clone)]
struct PreviewApp {
    count: State<i32>,
}

impl Default for PreviewApp {
    fn default() -> Self {
        Self {
            count: State::initial(StateId::new(0)),
        }
    }
}

impl PreviewApp {
    fn content(&self) -> impl View + Clone + use<> {
        vstack! {
            Text::new("ScarletUI Preview").font_size(28.0),
            Text::new(format!("Count: {}", self.count.get())).font_size(20.0),
            Button::new("Increment").on_click({
                let count = self.count.clone();
                move || count.set(count.get() + 1)
            }),
        }
        .spacing(12.0)
        .padding(20.0)
    }
}

impl View for PreviewApp {
    fn create_element(&self) -> Box<dyn Element> {
        self.content().create_element()
    }

    fn listenables(&self) -> Vec<&dyn Listenable> {
        vec![&self.count as &dyn Listenable]
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
}

impl Application for PreviewApp {
    fn scenes(&self) -> impl Scene {
        WindowGroup::new(
            "main",
            Window::new("Preview Demo", self.content()).size(Size::new(420.0, 260.0)),
        )
    }
}

#[scarlet_ui::preview(width = 420.0, height = 260.0)]
fn counter_preview() -> impl View + Clone {
    PreviewApp::default()
}

#[scarlet_ui::preview(width = 320.0, height = 180.0)]
fn button_preview() -> impl View + Clone {
    Button::new("Standalone Button").padding(20.0)
}

fn main() -> scarlet_ui::Result<()> {
    let mut app = PreviewApp::default();
    app.run()
}
