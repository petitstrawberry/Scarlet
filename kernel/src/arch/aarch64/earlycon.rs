use crate::boot::limine::FRAMEBUFFER_REQUEST;

pub fn early_putc(c: u8) {
    crate::earlyfb::putc(c);
}

pub fn early_console_init() {
    if crate::earlyfb::is_initialized() {
        return;
    }

    let Some(response) = FRAMEBUFFER_REQUEST.get_response() else {
        return;
    };
    let Some(framebuffer) = response.framebuffers().next() else {
        return;
    };

    crate::earlyfb::init(&framebuffer);
}

pub fn early_console_write(s: &str) {
    crate::earlyfb::write_str(s);
}
