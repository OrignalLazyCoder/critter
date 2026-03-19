use ratatui::style::Color;

#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Color,
    pub bg1: Color,
    pub bg3: Color,
    pub ln: Color,
    pub tx: Color,
    pub tx2: Color,
    pub tx3: Color,
    pub green: Color,
    pub amber: Color,
    pub coral: Color,
    pub sky: Color,
    pub violet: Color,
    pub rose: Color,
}

pub fn palette(truecolor: bool) -> Palette {
    if truecolor {
        Palette {
            bg: Color::Rgb(10, 10, 8),
            bg1: Color::Rgb(17, 17, 16),
            bg3: Color::Rgb(34, 34, 32),
            ln: Color::Rgb(42, 42, 38),
            tx: Color::Rgb(222, 218, 208),
            tx2: Color::Rgb(138, 136, 128),
            tx3: Color::Rgb(74, 74, 70),
            green: Color::Rgb(109, 200, 122),
            amber: Color::Rgb(200, 160, 64),
            coral: Color::Rgb(204, 106, 82),
            sky: Color::Rgb(90, 174, 200),
            violet: Color::Rgb(146, 120, 200),
            rose: Color::Rgb(196, 104, 136),
        }
    } else {
        Palette {
            bg: Color::Black,
            bg1: Color::DarkGray,
            bg3: Color::DarkGray,
            ln: Color::Gray,
            tx: Color::White,
            tx2: Color::Gray,
            tx3: Color::DarkGray,
            green: Color::Green,
            amber: Color::Yellow,
            coral: Color::Red,
            sky: Color::Cyan,
            violet: Color::Magenta,
            rose: Color::LightMagenta,
        }
    }
}
