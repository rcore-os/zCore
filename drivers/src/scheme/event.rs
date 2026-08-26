use crate::utils::EventHandler;

pub trait EventScheme {
    type Event;

    /// Trigger the event manually and call its handler immediately.
    fn trigger(&self, event: Self::Event);

    /// Subscribe events, call the `handler` when an input event occurs.
    /// If `once` is true, unsubscribe automatically after handling.
    /// Returns a subscription id for [`Self::unsubscribe`].
    fn subscribe(&self, handler: EventHandler<Self::Event>, once: bool) -> Option<u64>;

    /// Drop a parked waker subscription. Required when an async poll future is
    /// Ready/`Drop`ed before the once-handler fires.
    fn unsubscribe(&self, id: u64);
}

macro_rules! impl_event_scheme {
    ($struct:ident $(, $event_ty:ty)?) => {
        impl_event_scheme!(@impl_base $struct $(, $event_ty)?);
    };
    ($struct:ident<'_> $(, $event_ty:ty)?) => {
        impl_event_scheme!(@impl_base $struct<'_> $(, $event_ty)?);
    };
    ($struct:ident < $($types:ident),* > $(where $($preds:tt)+)? $(, $event_ty:ty)?) => {
        impl_event_scheme!(@impl_base $struct < $($types),* > $(where $($preds)+)? $(, $event_ty)?);
    };

    (@impl_base $struct:ident $(, $event_ty:ty)?) => {
        impl $crate::scheme::EventScheme for $struct {
            impl_event_scheme!(@impl_body $(, $event_ty)?);
        }
    };
    (@impl_base $struct:ident<'_> $(, $event_ty:ty)?) => {
        impl $crate::scheme::EventScheme for $struct<'_> {
            impl_event_scheme!(@impl_body $(, $event_ty)?);
        }
    };
    (@impl_base $struct:ident < $($types:ident),* > $(where $($preds:tt)+)? $(, $event_ty:ty)?) => {
        impl < $($types),* > $crate::scheme::EventScheme for $struct < $($types),* >
            $(where $($preds)+)?
        {
            impl_event_scheme!(@impl_body $(, $event_ty)?);
        }
    };

    (@impl_assoc_type) => {
        type Event = ();
    };
    (@impl_assoc_type, $event_ty:ty) => {
        type Event = $event_ty;
    };
    (@impl_body $(, $event_ty:ty)?) => {
        impl_event_scheme!(@impl_assoc_type $(, $event_ty)?);

        #[inline]
        fn trigger(&self, event: Self::Event) {
            self.listener.trigger(event);
        }

        #[inline]
        fn subscribe(&self, handler: $crate::utils::EventHandler<Self::Event>, once: bool) -> Option<u64> {
            self.listener.subscribe(handler, once)
        }

        #[inline]
        fn unsubscribe(&self, id: u64) {
            self.listener.unsubscribe(id);
        }
    };
}
