mod scrollable;
mod scrollable_mask;
mod scrollbar;

pub trait ScrollableElement: Sized {
	fn vertical_scrollbar<T>(self, _state: &T) -> Self {
		self
	}
}

impl<T> ScrollableElement for T {}

pub use scrollable::*;
pub use scrollable_mask::*;
pub use scrollbar::*;
