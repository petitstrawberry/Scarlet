use scarlet_ui::{prelude::*, vstack};
use std::{env, fs, path::PathBuf};

#[derive(Clone)]
struct AppDemo {
    message: String,
    count: State<u32>,
}

impl AppDemo {
    fn content(&self) -> impl View + Clone + use<> {
        vstack! {
            Text::new(env!("APP_NAME")).font_size(28.0),
            Text::new(self.message.clone()).font_size(16.0),
            Text::new(format!("Clicks: {}", self.count.get())).font_size(20.0),
            Button::new("Click me").on_click({
                let count = self.count.clone();
                move || count.set(count.get() + 1)
            }),
        }
        .spacing(16.0)
        .padding(28.0)
    }
}

impl View for AppDemo {
    fn create_element(&self) -> Box<dyn Element> {
        self.content().create_element()
    }

    fn listenables(&self) -> Vec<&dyn Listenable> {
        vec![&self.count]
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
}

impl Application for AppDemo {
    fn scenes(&self) -> impl Scene {
        WindowGroup::new(
            "main",
            Window::new(env!("APP_NAME"), self.clone())
                .app_id(env!("APP_ID"))
                .size(Size::new(520.0, 300.0)),
        )
    }
}

fn resource_path() -> std::result::Result<PathBuf, String> {
    // stemd expands the Scarlet bundle-relative Exec into an absolute path.
    let executable = PathBuf::from(env::args().next().ok_or("missing executable path")?);
    if executable.is_absolute() {
        if let Some(root) = executable.parent().and_then(|bin| bin.parent()) {
            let resource = root.join("resources/message.txt");
            if resource.is_file() {
                return Ok(resource);
            }
        }
    }
    #[cfg(not(target_os = "scarlet"))]
    return Ok(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/message.txt"));
    #[cfg(target_os = "scarlet")]
    Err("App Demo requires its packaged resources/message.txt".into())
}

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let message = fs::read_to_string(resource_path().map_err(std::io::Error::other)?)?;
    let mut app = AppDemo {
        message: message.trim().into(),
        count: State::initial(StateId::new(1)),
    };
    app.run()
        .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
    Ok(())
}
