use proc_macro::TokenStream;
use quote::{format_ident, quote};

#[proc_macro_attribute]
pub fn grpc_component(attr: TokenStream, item: TokenStream) -> TokenStream {
    let server_type = syn::parse_macro_input!(attr as syn::Path);
    let http_impl_struct = syn::parse_macro_input!(item as syn::ItemStruct);
    let http_impl_struct_name = &http_impl_struct.ident;
    let wasi_implementor = format_ident!("{}GrpcServer", http_impl_struct_name);

    quote!(
        #[doc(hidden)]
        mod __wasi_grpc {
            struct #wasi_implementor;

            ::wasi::http::proxy::export!(#wasi_implementor);

            impl ::wasi::exports::http::incoming_handler::Guest for #wasi_implementor {
                fn handle(request: ::wasi::exports::http::incoming_handler::IncomingRequest, response_out: ::wasi::exports::http::incoming_handler::ResponseOutparam) {
                    static INIT: ::std::sync::Once = ::std::sync::Once::new();
                    INIT.call_once(|| {
                        println!(concat!(stringify!(#http_impl_struct_name), " gRPC component initialized and ready to handle requests"));
                    });
                    println!(concat!(stringify!(#http_impl_struct_name), " handling incoming request"));
                    let registry = ::wasi_hyperium::poll::Poller::default();
                    let server = super::#server_type::new(super::#http_impl_struct_name);
                    let e = ::wasi_hyperium::hyperium1::handle_service_call(server, request, response_out, registry);
                    e.unwrap();
                }
            }
        }

        #http_impl_struct
    )
    .into()
}
