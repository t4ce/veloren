// Exercise both the public and Conrod-internal derives without depending on
// the full UI crate. The generated public implementations use this crate name.
extern crate self as conrod_core;
#[macro_use]
extern crate conrod_derive;

use std::any::Any;

pub mod widget {
    #[derive(Default)]
    pub struct CommonBuilder(pub u32);

    pub trait Common {
        fn common(&self) -> &CommonBuilder;
        fn common_mut(&mut self) -> &mut CommonBuilder;
    }
}

pub struct WidgetDefault<T> {
    pub style: T,
}

#[derive(Default)]
pub struct Theme {
    style: Option<Box<dyn Any>>,
}

impl Theme {
    pub fn widget_style<T: 'static>(&self) -> Option<&WidgetDefault<T>> {
        self.style.as_ref()?.downcast_ref()
    }
}

#[derive(WidgetCommon)]
struct PublicWidget<T> {
    #[conrod(common_builder,)]
    common: widget::CommonBuilder,
    payload: T,
}

#[derive(WidgetCommon_)]
struct InternalWidget<T> {
    #[conrod(common_builder)]
    common: widget::CommonBuilder,
    payload: T,
}

#[derive(Clone, Copy, WidgetStyle)]
struct PublicStyle<T: Copy + 'static> {
    #[conrod(default = "42")]
    value: Option<u32>,
    // Fields without a Conrod attribute must not generate getters.
    payload: T,
}

#[derive(Clone, Copy, WidgetStyle_)]
struct InternalStyle {
    #[conrod(default = "21 * 2",)]
    value: Option<u32>,
}

#[test]
fn common_derives_access_the_marked_field() {
    use widget::Common;

    let mut public = PublicWidget {
        common: widget::CommonBuilder(1),
        payload: "public",
    };
    let mut internal = InternalWidget {
        common: widget::CommonBuilder(2),
        payload: "internal",
    };
    assert_eq!(public.common().0, 1);
    assert_eq!(internal.common().0, 2);
    public.common_mut().0 = 3;
    internal.common_mut().0 = 4;
    assert_eq!(public.common().0, 3);
    assert_eq!(internal.common().0, 4);
    assert_eq!(public.payload, "public");
    assert_eq!(internal.payload, "internal");
}

#[test]
fn style_derives_preserve_override_theme_and_expression_defaults() {
    let public = PublicStyle { value: None, payload: () };
    let internal = InternalStyle { value: None };
    let empty_theme = Theme::default();
    assert_eq!(public.value(&empty_theme), 42);
    assert_eq!(internal.value(&empty_theme), 42);

    let public_theme = Theme {
        style: Some(Box::new(WidgetDefault {
            style: PublicStyle { value: Some(7), payload: () },
        })),
    };
    let internal_theme = Theme {
        style: Some(Box::new(WidgetDefault {
            style: InternalStyle { value: Some(8) },
        })),
    };
    assert_eq!(public.value(&public_theme), 7);
    assert_eq!(internal.value(&internal_theme), 8);
    assert_eq!(PublicStyle { value: Some(9), ..public }.value(&public_theme), 9);
    assert_eq!(InternalStyle { value: Some(10) }.value(&internal_theme), 10);
    assert_eq!(public.payload, ());
}
