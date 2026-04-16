use proc_macro::TokenStream;
use quote::{format_ident, quote};

/// Marks a struct as handling gRPC requests for the nominated server type.
/// You must implement the corresponding service trait on the struct.
///
/// Usage:
///
/// ```ignore
/// pub mod routeguide {
///     tonic::include_proto!("routeguide");
/// }
///
/// use routeguide::route_guide_server::{RouteGuide, RouteGuideServer};
///
/// #[grpc_component(RouteGuideServer)]
/// struct MyRouteGuide;
///
/// #[tonic::async_trait]
/// impl RouteGuide for MyRouteGuide {
///     // ...
/// }
/// ```
///
/// The generated code depends on the following crates:
///
/// * wasi
/// * wasi-hyperium
///
/// You must add references to these to your Cargo.toml.
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
                    match ::wasi_hyperium::hyperium1::handle_service_call(server, request, response_out, registry) {
                        Ok(()) => eprintln!(concat!(stringify!(#http_impl_struct_name), " request completed successfully")),
                        Err(e) => eprintln!(concat!(stringify!(#http_impl_struct_name), " handle_service_call error: {:?}"), e),
                    }
                }
            }
        }

        #http_impl_struct
    )
    .into()
}
