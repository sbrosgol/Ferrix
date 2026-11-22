//! Draws a page: its whole rendering (exact, or a low-resolution base for large pages) scaled
//! to the page's size, then any sharp tiles over it. Tiles arrive as the view settles, so a
//! page is never blank once its base is in, and sharpens in place (architecture §5.3).

use iced::{
    Rectangle, Renderer, Theme, mouse,
    widget::canvas::{self, Frame, Geometry, Image},
};

use crate::{app::Message, render::PageImage};

#[derive(Debug)]
pub(crate) struct PagePaper<'a> {
    pub(crate) whole: Option<&'a PageImage>,
    /// Sharp tiles in page logical pixels.
    pub(crate) tiles: Vec<(Rectangle, iced::widget::image::Handle)>,
}

impl canvas::Program<Message> for PagePaper<'_> {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        if let Some(whole) = self.whole {
            let (sx, sy) = (
                bounds.width / whole.logical.width,
                bounds.height / whole.logical.height,
            );
            // The rendering is already cut into ≤512 px tiles (the software renderer drops
            // larger images); stretch each into place.
            let mut y = 0.0;
            for row in &whole.rows {
                let mut x = 0.0;
                let mut row_height: f32 = 0.0;
                for (handle, size) in row {
                    let rect = Rectangle::new(
                        iced::Point::new(x * sx, y * sy),
                        iced::Size::new(size.width * sx, size.height * sy),
                    );
                    frame.draw_image(rect, Image::new(handle.clone()));
                    x += size.width;
                    row_height = row_height.max(size.height);
                }
                y += row_height;
            }
        }
        for (rect, handle) in &self.tiles {
            frame.draw_image(*rect, Image::new(handle.clone()));
        }
        vec![frame.into_geometry()]
    }
}
