use std::{borrow::Cow, io};
use wreq::{
    http2::{
        Http2Options, PseudoId, PseudoOrder, SettingId, SettingsOrder, StreamDependency, StreamId,
    },
    tls::{
        AlpnProtocol, AlpsProtocol, CertificateFailureAlert, ExtensionType, KeyShare, TlsOptions,
        TlsVersion,
        compress::{CertificateCompressionAlgorithm, CertificateCompressor, Codec},
    },
};

#[derive(Debug)]
struct BrotliCertificateCompressor;
static BROTLI: BrotliCertificateCompressor = BrotliCertificateCompressor;

impl CertificateCompressor for BrotliCertificateCompressor {
    fn compress(&self) -> Codec {
        Codec::Pointer(|input, mut output| {
            brotli::BrotliCompress(
                &mut io::Cursor::new(input),
                &mut output,
                &brotli::enc::BrotliEncoderParams::default(),
            )
            .map(|_| ())
        })
    }
    fn decompress(&self) -> Codec {
        Codec::Pointer(|input, mut output| {
            brotli::BrotliDecompress(&mut io::Cursor::new(input), &mut output)
        })
    }
    fn algorithm(&self) -> CertificateCompressionAlgorithm {
        CertificateCompressionAlgorithm::BROTLI
    }
}

pub(super) fn tls_options() -> TlsOptions {
    TlsOptions::builder()
        .min_tls_version(TlsVersion::TLS_1_2)
        .max_tls_version(TlsVersion::TLS_1_3)
        .alpn_protocols([AlpnProtocol::HTTP2, AlpnProtocol::HTTP1])
        .alps_protocols([AlpsProtocol::HTTP2])
        .alps_use_new_codepoint(true)
        .session_ticket(true)
        .pre_shared_key(false)
        .enable_ech_grease(true)
        .permute_extensions(false)
        .grease_enabled(true)
        .enable_ocsp_stapling(true)
        .enable_signed_cert_timestamps(true)
        .psk_dhe_ke(true)
        .renegotiation(true)
        .curves_list("X25519MLKEM768:X25519:P-256:P-384")
        .key_shares(vec![KeyShare::X25519_MLKEM768, KeyShare::X25519])
        .preserve_tls13_cipher_list(true)
        .cipher_list("TLS_AES_128_GCM_SHA256:TLS_AES_256_GCM_SHA384:TLS_CHACHA20_POLY1305_SHA256:ECDHE-ECDSA-AES128-GCM-SHA256:ECDHE-RSA-AES128-GCM-SHA256:ECDHE-ECDSA-AES256-GCM-SHA384:ECDHE-RSA-AES256-GCM-SHA384:ECDHE-ECDSA-CHACHA20-POLY1305:ECDHE-RSA-CHACHA20-POLY1305:ECDHE-RSA-AES128-SHA:ECDHE-RSA-AES256-SHA:AES128-GCM-SHA256:AES256-GCM-SHA384:AES128-SHA:AES256-SHA")
        .sigalgs_list("ecdsa_secp256r1_sha256:rsa_pss_rsae_sha256:rsa_pkcs1_sha256:ecdsa_secp384r1_sha384:rsa_pss_rsae_sha384:rsa_pkcs1_sha384:rsa_pss_rsae_sha512:rsa_pkcs1_sha512")
        .certificate_compressors(vec![&BROTLI as &'static dyn CertificateCompressor])
        .extension_permutation([51_u16, 0, 17613, 65281, 10, 27, 35, 5, 23, 43, 13, 18, 11, 65037, 16, 45, 51764].into_iter().map(ExtensionType::from).collect::<Vec<_>>())
        .requested_trust_anchors(Cow::Borrowed(&[] as &[u8]))
        .certificate_failure_alert(CertificateFailureAlert::BadCertificate)
        .aes_hw_override(true)
        .random_aes_hw_override(false)
        .build()
}

pub(super) fn http2_options() -> Http2Options {
    Http2Options::builder()
        .header_table_size(65536)
        .enable_push(false)
        .initial_window_size(6291456)
        .max_header_list_size(262144)
        .settings_order(
            SettingsOrder::builder()
                .extend([
                    SettingId::HeaderTableSize,
                    SettingId::EnablePush,
                    SettingId::InitialWindowSize,
                    SettingId::MaxHeaderListSize,
                ])
                .build(),
        )
        .initial_connection_window_size(15728640)
        .headers_pseudo_order(
            PseudoOrder::builder()
                .extend([
                    PseudoId::Method,
                    PseudoId::Authority,
                    PseudoId::Scheme,
                    PseudoId::Path,
                ])
                .build(),
        )
        .headers_stream_dependency(StreamDependency::new(StreamId::from(0), 255, true))
        .adaptive_window(false)
        .keep_alive_interval(None)
        .build()
}
