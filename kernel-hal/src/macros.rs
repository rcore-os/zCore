macro_rules! hal_fn_def {
    (
        $(
            $vis:vis mod $mod_name:ident $( : $base:path )? {
                $($fn:tt)*
            }
        )+
    ) => {
        $(
            $vis mod $mod_name {
                #![allow(unused_imports)]
                $( pub use $base::*; )?
                use super::*;
                pub(crate) trait __HalTrait {
                    __hal_fn_default_imp! {
                        $($fn)*
                    }
                }
                pub(crate) struct __HalImpl;
                __hal_fn_export! {
                    $($fn)*
                }
            }
        )+
    };
    () => {};
}

macro_rules! hal_fn_impl {
    (
        $(
            impl mod $mod_name:ident$(::$mod_name_more:ident)* {
                $($fn:item)*
            }
        )+
    ) => {
        $(
            __hal_fn_impl_no_export! {
                impl mod $mod_name$(::$mod_name_more)* {
                    $($fn)*
                }
            }
            pub use $mod_name$(::$mod_name_more)*::*;
        )+
    };
    () => {};
}

macro_rules! hal_fn_impl_default {
    (
        $( $mod_name:ident$(::$mod_name_more:ident)* ),*
    ) => {
        __hal_fn_impl_no_export! {
            $( impl mod $mod_name$(::$mod_name_more)* {} )*
        }
    }
}

macro_rules! __hal_fn_default_imp {
    (
        $(#[$inner:ident $($args:tt)*])*
        fn $fn:ident ( $($arg:ident : $type:ty),* ) $( -> $ret:ty )?;
        $($tail:tt)*
    ) => {
        $(#[$inner $($args)*])*
        #[allow(unused_variables)]
        fn $fn ( $($arg : $type),* ) $( -> $ret )? {
            unimplemented!();
        }
        __hal_fn_default_imp! {
            $($tail)*
        }
    };
    (
        $(#[$inner:ident $($args:tt)*])*
        fn $fn:ident ( $($arg:ident : $type:ty),* ) $( -> $ret:ty )? $body:block
        $($tail:tt)*
    ) => {
        $(#[$inner $($args)*])*
        fn $fn ( $($arg : $type),* ) $( -> $ret )? $body
        __hal_fn_default_imp! {
            $($tail)*
        }
    };
    () => {};
}

macro_rules! __hal_fn_export {
    (
        $(#[$inner:ident $($args:tt)*])*
        fn $fn:ident ( $($arg:ident : $type:ty),* ) $( -> $ret:ty )?;
        $($tail:tt)*
    ) => {
        $(#[$inner $($args)*])*
        #[allow(dead_code)]
        pub fn $fn ( $($arg : $type),* ) $( -> $ret )? {
            __HalImpl::$fn( $($arg),* )
        }
        __hal_fn_export! {
            $($tail)*
        }
    };
    (
        $(#[$inner:ident $($args:tt)*])*
        fn $fn:ident ( $($arg:ident : $type:ty),* ) $( -> $ret:ty )? $body:block
        $($tail:tt)*
    ) => {
        $(#[$inner $($args)*])*
        #[allow(dead_code)]
        pub fn $fn ( $($arg : $type),* ) $( -> $ret )? {
            __HalImpl::$fn( $($arg),* )
        }
        __hal_fn_export! {
            $($tail)*
        }
    };
    () => {};
}

macro_rules! __hal_fn_impl_no_export {
    (
        $(
            impl mod $mod_name:ident$(::$mod_name_more:ident)* {
                $($fn:item)*
            }
        )+
    ) => {
        $(
            impl $mod_name$(::$mod_name_more)*::__HalTrait for $mod_name$(::$mod_name_more)*::__HalImpl {
                $($fn)*
            }
        )+
    };
    () => {};
}

#[cfg(test)]
mod test {
    mod base {
        pub const BASE: usize = 0x2333;
    }
    hal_fn_def! {
        mod mod0: self::base {
            fn test() -> f32 { 1.0 }
        }
        mod mod1 {
            fn foo() -> usize { 0 }
            fn bar(a: usize) -> usize { a }
            fn unimp();
        }
    }
    hal_fn_impl! {
        impl mod self::mod1 {
            fn foo() -> usize { 233 }
        }
    }
    hal_fn_impl_default!(mod0);

    #[test]
    fn test_hal_fn_marco() {
        assert_eq!(mod0::BASE, 0x2333);
        assert_eq!(mod0::test(), 1.0);
        assert_eq!(mod1::foo(), 233);
        assert_eq!(mod1::bar(0), 0);
        // base::unimp(); // unimplemented!
    }
}
