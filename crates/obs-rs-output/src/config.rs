//! Typed production-stream settings shared by frontends and output backends.

use std::{fmt, path::PathBuf};
use url::Url;
use zeroize::Zeroize;

/// Text whose formatting traits never reveal its contents.
#[derive(Clone, Default, Eq, PartialEq)]
pub struct SecretString(String);

impl SecretString {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Explicitly exposes the value at the narrow boundary that consumes it.
    #[must_use]
    pub fn expose_secret(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretString([REDACTED])")
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

impl Drop for SecretString {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum StreamProtocol {
    #[default]
    Rtmp,
    Rtmps,
    Srt,
    Whip,
    Hls,
    Rist,
    Reference,
}

impl StreamProtocol {
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Rtmp => "rtmp",
            Self::Rtmps => "rtmps",
            Self::Srt => "srt",
            Self::Whip => "whip",
            Self::Hls => "hls",
            Self::Rist => "rist",
            Self::Reference => "reference",
        }
    }

    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        match id.trim().to_ascii_lowercase().as_str() {
            "rtmp" => Some(Self::Rtmp),
            "rtmps" => Some(Self::Rtmps),
            "srt" => Some(Self::Srt),
            "whip" | "webrtc" => Some(Self::Whip),
            "hls" => Some(Self::Hls),
            "rist" => Some(Self::Rist),
            "reference" => Some(Self::Reference),
            _ => None,
        }
    }
}

/// A semantic destination retained without materializing credentials into a URL.
#[derive(Clone, Eq, PartialEq)]
pub enum StreamTarget {
    Rtmp(RtmpConfig),
    Rtmps(RtmpConfig),
    Srt(SrtConfig),
    Whip(WhipConfig),
    Hls(HlsConfig),
    Rist(RistConfig),
    Reference { address: String },
}

impl StreamTarget {
    #[must_use]
    pub const fn protocol(&self) -> StreamProtocol {
        match self {
            Self::Rtmp(_) => StreamProtocol::Rtmp,
            Self::Rtmps(_) => StreamProtocol::Rtmps,
            Self::Srt(_) => StreamProtocol::Srt,
            Self::Whip(_) => StreamProtocol::Whip,
            Self::Hls(_) => StreamProtocol::Hls,
            Self::Rist(_) => StreamProtocol::Rist,
            Self::Reference { .. } => StreamProtocol::Reference,
        }
    }

    /// Materializes the destination only for the transport connection call.
    #[must_use]
    pub fn endpoint(&self) -> Option<String> {
        match self {
            Self::Rtmp(config) => config.endpoint(StreamProtocol::Rtmp),
            Self::Rtmps(config) => config.endpoint(StreamProtocol::Rtmps),
            Self::Srt(config) => config.endpoint(),
            Self::Whip(config) => nonempty(&config.endpoint),
            Self::Hls(config) => nonempty(config.directory.to_str()?),
            Self::Rist(config) => config.endpoint(),
            Self::Reference { address } => nonempty(address),
        }
    }
}

/// One bounded built-in service entry for the RTMP/RTMPS settings page.
///
/// The server is the first usable ingest entry from the pinned OBS service
/// catalog; the server field remains editable for regional or custom ingest.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StreamingServicePreset {
    id: &'static str,
    display_name: &'static str,
    protocol: StreamProtocol,
    default_server: &'static str,
    stream_key_link: Option<&'static str>,
}

impl StreamingServicePreset {
    #[must_use]
    pub const fn id(self) -> &'static str {
        self.id
    }

    #[must_use]
    pub const fn display_name(self) -> &'static str {
        self.display_name
    }

    #[must_use]
    pub const fn protocol(self) -> StreamProtocol {
        self.protocol
    }

    #[must_use]
    pub const fn default_server(self) -> &'static str {
        self.default_server
    }

    #[must_use]
    pub const fn stream_key_link(self) -> Option<&'static str> {
        self.stream_key_link
    }

    #[must_use]
    pub fn matches(self, value: &str) -> bool {
        let value = value.trim();
        self.id.eq_ignore_ascii_case(value) || self.display_name.eq_ignore_ascii_case(value)
    }
}

/// Pinned RTMP-family portion of the OBS 32.2.2 service catalog.
///
/// The source catalog contains 84 services. The three entries whose primary
/// workflow is HLS or an HTTP/API setup are intentionally not represented here:
/// this picker configures the typed RTMP/RTMPS target, and must not advertise a
/// service whose endpoint cannot be consumed by that target. This remains
/// intentionally bounded and compile-time owned; a future signed catalog update
/// can replace it without coupling service metadata to a native transport
/// implementation.
pub const RTMP_SERVICE_PRESETS: [StreamingServicePreset; 82] = [
    StreamingServicePreset { id: "custom", display_name: "Custom", protocol: StreamProtocol::Rtmp, default_server: "127.0.0.1/live", stream_key_link: None },
    StreamingServicePreset { id: "twitch", display_name: "Twitch", protocol: StreamProtocol::Rtmp, default_server: "live-hkg.twitch.tv/app", stream_key_link: Some("https://dashboard.twitch.tv/settings/stream") },
    StreamingServicePreset { id: "youtube-rtmps", display_name: "YouTube - RTMPS", protocol: StreamProtocol::Rtmps, default_server: "a.rtmps.youtube.com:443/live2", stream_key_link: Some("https://www.youtube.com/live_dashboard") },
    StreamingServicePreset { id: "loola-tv", display_name: "Loola.tv", protocol: StreamProtocol::Rtmp, default_server: "rtmp.loola.tv/push", stream_key_link: None },
    StreamingServicePreset { id: "lovecast", display_name: "Lovecast", protocol: StreamProtocol::Rtmp, default_server: "live-a.lovecastapp.com:5222/app", stream_key_link: None },
    StreamingServicePreset { id: "luzento-com-rtmp", display_name: "Luzento.com - RTMP", protocol: StreamProtocol::Rtmp, default_server: "ingest.luzento.com/live", stream_key_link: Some("https://cms.luzento.com/dashboard/stream-key?from=OBS") },
    StreamingServicePreset { id: "web-tv", display_name: "Web.TV", protocol: StreamProtocol::Rtmp, default_server: "live3.origins.web.tv/liveext", stream_key_link: None },
    StreamingServicePreset { id: "goodgame-ru", display_name: "GoodGame.ru", protocol: StreamProtocol::Rtmp, default_server: "msk.goodgame.ru:1940/live", stream_key_link: None },
    StreamingServicePreset { id: "vaughn-live-instagib", display_name: "Vaughn Live / iNSTAGIB", protocol: StreamProtocol::Rtmp, default_server: "live-iad.vaughnsoft.net/live", stream_key_link: None },
    StreamingServicePreset { id: "breakers-tv", display_name: "Breakers.TV", protocol: StreamProtocol::Rtmp, default_server: "live-iad.vaughnsoft.net/live", stream_key_link: None },
    StreamingServicePreset { id: "facebook-live", display_name: "Facebook Live", protocol: StreamProtocol::Rtmps, default_server: "rtmp-api.facebook.com:443/rtmp/", stream_key_link: Some("https://www.facebook.com/live/producer?ref=OBS") },
    StreamingServicePreset { id: "restream", display_name: "Restream.io", protocol: StreamProtocol::Rtmp, default_server: "live.restream.io/live", stream_key_link: Some("https://restream.io/settings/streaming-setup?from=OBS") },
    StreamingServicePreset { id: "castr-io", display_name: "Castr.io", protocol: StreamProtocol::Rtmp, default_server: "cg.castr.io/static", stream_key_link: None },
    StreamingServicePreset { id: "boomstream", display_name: "Boomstream", protocol: StreamProtocol::Rtmp, default_server: "live.boomstream.com/live", stream_key_link: None },
    StreamingServicePreset { id: "meridix-live-sports-platform", display_name: "Meridix Live Sports Platform", protocol: StreamProtocol::Rtmp, default_server: "publish.meridix.com/live", stream_key_link: None },
    StreamingServicePreset { id: "soop-korea", display_name: "SOOP Korea", protocol: StreamProtocol::Rtmp, default_server: "stream.sooplive.co.kr/app/", stream_key_link: None },
    StreamingServicePreset { id: "cam4", display_name: "CAM4", protocol: StreamProtocol::Rtmp, default_server: "origin.cam4.com/cam4-origin-live", stream_key_link: None },
    StreamingServicePreset { id: "eplay", display_name: "ePlay", protocol: StreamProtocol::Rtmp, default_server: "live.eplay.link/origin", stream_key_link: None },
    StreamingServicePreset { id: "picarto", display_name: "Picarto", protocol: StreamProtocol::Rtmp, default_server: "live.us.picarto.tv/golive", stream_key_link: None },
    StreamingServicePreset { id: "uscreen", display_name: "Uscreen", protocol: StreamProtocol::Rtmp, default_server: "global-live.uscreen.app:5222/app", stream_key_link: None },
    StreamingServicePreset { id: "stripchat", display_name: "Stripchat", protocol: StreamProtocol::Rtmp, default_server: "live.doppiocdn.com/ext", stream_key_link: None },
    StreamingServicePreset { id: "camsoda", display_name: "CamSoda", protocol: StreamProtocol::Rtmp, default_server: "obs-ingest-na.livemediahost.com/cam_obs", stream_key_link: None },
    StreamingServicePreset { id: "chaturbate", display_name: "Chaturbate", protocol: StreamProtocol::Rtmp, default_server: "global.live.mmcdn.com/live-origin", stream_key_link: Some("https://chaturbate.com/b/?useExternalSoftware=true") },
    StreamingServicePreset { id: "wpstream", display_name: "WpStream", protocol: StreamProtocol::Rtmp, default_server: "ingest.wpstream.net/golive", stream_key_link: Some("https://wpstream.net/obs-get-stream-key") },
    StreamingServicePreset { id: "twitter", display_name: "Twitter", protocol: StreamProtocol::Rtmp, default_server: "ca.pscp.tv:80/x", stream_key_link: Some("https://studio.twitter.com/producer/sources") },
    StreamingServicePreset { id: "switchboard-live", display_name: "Switchboard Live", protocol: StreamProtocol::Rtmps, default_server: "live.sb.zone:443/live", stream_key_link: None },
    StreamingServicePreset { id: "eventials", display_name: "Eventials", protocol: StreamProtocol::Rtmp, default_server: "transmission.eventials.com/eventialsLiveOrigin", stream_key_link: None },
    StreamingServicePreset { id: "eventlive-pro", display_name: "EventLive.pro", protocol: StreamProtocol::Rtmp, default_server: "go.eventlive.pro/live", stream_key_link: None },
    StreamingServicePreset { id: "lahzenegar-streamg", display_name: "Lahzenegar - StreamG | لحظه‌نگار - استریمجی", protocol: StreamProtocol::Rtmp, default_server: "rtmp.lahzecdn.com/pro", stream_key_link: None },
    StreamingServicePreset { id: "mylive", display_name: "MyLive", protocol: StreamProtocol::Rtmp, default_server: "stream.mylive.in.th/live", stream_key_link: None },
    StreamingServicePreset { id: "trovo", display_name: "Trovo", protocol: StreamProtocol::Rtmp, default_server: "livepush.trovo.live/live/", stream_key_link: Some("https://studio.trovo.live/mychannel/stream") },
    StreamingServicePreset { id: "mixcloud", display_name: "Mixcloud", protocol: StreamProtocol::Rtmp, default_server: "rtmp.mixcloud.com/broadcast", stream_key_link: None },
    StreamingServicePreset { id: "sermonaudio-cloud", display_name: "SermonAudio Cloud", protocol: StreamProtocol::Rtmp, default_server: "webcast.sermonaudio.com/sa", stream_key_link: None },
    StreamingServicePreset { id: "vimeo", display_name: "Vimeo", protocol: StreamProtocol::Rtmp, default_server: "rtmp.cloud.vimeo.com/live", stream_key_link: None },
    StreamingServicePreset { id: "aparat", display_name: "Aparat", protocol: StreamProtocol::Rtmp, default_server: "rtmp.cdn.asset.aparat.com:443/event", stream_key_link: None },
    StreamingServicePreset { id: "kakaotv", display_name: "KakaoTV", protocol: StreamProtocol::Rtmp, default_server: "rtmp.play.kakao.com/kakaotv", stream_key_link: None },
    StreamingServicePreset { id: "piczel-tv", display_name: "Piczel.tv", protocol: StreamProtocol::Rtmp, default_server: "boston.piczel.tv/live", stream_key_link: None },
    StreamingServicePreset { id: "dlive", display_name: "DLive", protocol: StreamProtocol::Rtmp, default_server: "stream.dlive.tv/live", stream_key_link: None },
    StreamingServicePreset { id: "lightcast-com", display_name: "Lightcast.com", protocol: StreamProtocol::Rtmp, default_server: "ingest-na1.live.lightcast.com/in", stream_key_link: None },
    StreamingServicePreset { id: "bongacams", display_name: "Bongacams", protocol: StreamProtocol::Rtmp, default_server: "auto.origin.gnsbc.com:1934/live", stream_key_link: None },
    StreamingServicePreset { id: "onlyfans-com", display_name: "OnlyFans.com", protocol: StreamProtocol::Rtmp, default_server: "cloudbetastreaming.onlyfans.com/live", stream_key_link: Some("https://onlyfans.com/my/settings/other") },
    StreamingServicePreset { id: "steam", display_name: "Steam", protocol: StreamProtocol::Rtmp, default_server: "ingest-rtmp.broadcast.steamcontent.com/app", stream_key_link: None },
    StreamingServicePreset { id: "konduit-live", display_name: "Konduit.live", protocol: StreamProtocol::Rtmp, default_server: "rtmp.konduit.live/live", stream_key_link: None },
    StreamingServicePreset { id: "niconico", display_name: "niconico (ニコニコ生放送)", protocol: StreamProtocol::Rtmp, default_server: "liveorigin.dlive.nicovideo.jp/live/input", stream_key_link: None },
    StreamingServicePreset { id: "nimo-tv", display_name: "Nimo TV", protocol: StreamProtocol::Rtmp, default_server: "txpush.rtmp.nimo.tv/live/", stream_key_link: None },
    StreamingServicePreset { id: "xlovecam-com", display_name: "XLoveCam.com", protocol: StreamProtocol::Rtmp, default_server: "nl.eu.stream.xlove.com/performer-origin", stream_key_link: None },
    StreamingServicePreset { id: "angelthump", display_name: "AngelThump", protocol: StreamProtocol::Rtmp, default_server: "ingest.angelthump.com/live", stream_key_link: None },
    StreamingServicePreset { id: "api-video", display_name: "api.video", protocol: StreamProtocol::Rtmp, default_server: "broadcast.api.video/s", stream_key_link: None },
    StreamingServicePreset { id: "mux", display_name: "Mux", protocol: StreamProtocol::Rtmps, default_server: "global-live.mux.com:443/app", stream_key_link: None },
    StreamingServicePreset { id: "viloud", display_name: "Viloud", protocol: StreamProtocol::Rtmp, default_server: "live.viloud.tv:5222/app", stream_key_link: None },
    StreamingServicePreset { id: "myfreecams", display_name: "MyFreeCams", protocol: StreamProtocol::Rtmp, default_server: "publish.myfreecams.com/NxServer", stream_key_link: None },
    StreamingServicePreset { id: "polystreamer-com", display_name: "PolyStreamer.com", protocol: StreamProtocol::Rtmp, default_server: "live.polystreamer.com/live", stream_key_link: None },
    StreamingServicePreset { id: "openrec-tv-premium-member", display_name: "OPENREC.tv - Premium member (プレミアム会員)", protocol: StreamProtocol::Rtmp, default_server: "a.station.openrec.tv:1935/live1", stream_key_link: Some("https://www.openrec.tv/login?keep_login=true&url=https://www.openrec.tv/dashboard/live?from=obs") },
    StreamingServicePreset { id: "nanostream-cloud-bintu", display_name: "nanoStream Cloud / bintu", protocol: StreamProtocol::Rtmp, default_server: "bintu-stream.nanocosmos.de/live", stream_key_link: Some("https://bintu-cloud-frontend.nanocosmos.de/organisation") },
    StreamingServicePreset { id: "bilibili-live-rtmp-rtmp", display_name: "Bilibili Live - RTMP | 哔哩哔哩直播 - RTMP", protocol: StreamProtocol::Rtmp, default_server: "live-push.bilivideo.com/live-bvc/", stream_key_link: Some("https://link.bilibili.com/p/center/index#/my-room/start-live") },
    StreamingServicePreset { id: "boxcast", display_name: "BoxCast", protocol: StreamProtocol::Rtmp, default_server: "rtmp.boxcast.com/live", stream_key_link: Some("https://dashboard.boxcast.com/#/sources") },
    StreamingServicePreset { id: "disciple-media", display_name: "Disciple Media", protocol: StreamProtocol::Rtmp, default_server: "rtmp.disciplemedia.com/b-fme", stream_key_link: None },
    StreamingServicePreset { id: "jio-games", display_name: "Jio Games", protocol: StreamProtocol::Rtmp, default_server: "livepub1.api.engageapps.jio/live", stream_key_link: None },
    StreamingServicePreset { id: "kuaishou-live", display_name: "Kuaishou Live", protocol: StreamProtocol::Rtmp, default_server: "open-push.voip.yximgs.com/gifshow/", stream_key_link: Some("https://studio.kuaishou.com/live/list") },
    StreamingServicePreset { id: "phonelivestreaming", display_name: "PhoneLiveStreaming", protocol: StreamProtocol::Rtmp, default_server: "live.phonelivestreaming.com/live/", stream_key_link: Some("https://app.phonelivestreaming.com/media/rtmp") },
    StreamingServicePreset { id: "sympla", display_name: "Sympla", protocol: StreamProtocol::Rtmp, default_server: "rtmp.sympla.com.br:5222/app", stream_key_link: None },
    StreamingServicePreset { id: "livepush", display_name: "Livepush", protocol: StreamProtocol::Rtmp, default_server: "dc-global.livepush.io/live", stream_key_link: None },
    StreamingServicePreset { id: "vindral", display_name: "Vindral", protocol: StreamProtocol::Rtmps, default_server: "rtmp.global.cdn.vindral.com/publish", stream_key_link: Some("https://portal.cdn.vindral.com/channels") },
    StreamingServicePreset { id: "whowatch", display_name: "Whowatch (ふわっち)", protocol: StreamProtocol::Rtmp, default_server: "live.whowatch.tv/live/", stream_key_link: Some("https://whowatch.tv/publish") },
    StreamingServicePreset { id: "irltoolkit", display_name: "IRLToolkit", protocol: StreamProtocol::Rtmps, default_server: "stream.global.irl.run/ingest", stream_key_link: Some("https://irl.run/settings/ingest/") },
    StreamingServicePreset { id: "bitmovin", display_name: "Bitmovin", protocol: StreamProtocol::Rtmp, default_server: "live-input.bitmovin.com/streams", stream_key_link: Some("https://bitmovin.com/dashboard/streams?streamsTab=LIVE") },
    StreamingServicePreset { id: "enchant-events", display_name: "Enchant.events", protocol: StreamProtocol::Rtmps, default_server: "stream.enchant.cloud:443/live", stream_key_link: None },
    StreamingServicePreset { id: "joystick-tv", display_name: "Joystick.TV", protocol: StreamProtocol::Rtmp, default_server: "live.joystick.tv/live/", stream_key_link: Some("https://joystick.tv/stream-settings") },
    StreamingServicePreset { id: "livepeer-studio", display_name: "Livepeer Studio", protocol: StreamProtocol::Rtmp, default_server: "rtmp.livepeer.com/live", stream_key_link: Some("https://livepeer.studio/dashboard/streams") },
    StreamingServicePreset { id: "masterstream-ir", display_name: "MasterStream.iR | مستراستریم | ری استریم و استریم همزمان", protocol: StreamProtocol::Rtmp, default_server: "live1.masterstream.ir/live", stream_key_link: Some("https://masterstream.ir/control-panel/streaming") },
    StreamingServicePreset { id: "pandatv", display_name: "PandaTV | 팬더티비", protocol: StreamProtocol::Rtmp, default_server: "rtmp.pandalive.co.kr/app", stream_key_link: None },
    StreamingServicePreset { id: "vault-by-commanderroot", display_name: "Vault - by CommanderRoot", protocol: StreamProtocol::Rtmp, default_server: "ingest-eu-central.vault.root-space.eu/app", stream_key_link: Some("https://vault.root-space.eu/recordings") },
    StreamingServicePreset { id: "chzzk", display_name: "CHZZK", protocol: StreamProtocol::Rtmp, default_server: "global-rtmp.lip2.navercorp.com:8080/relay", stream_key_link: Some("https://studio.chzzk.naver.com/setting") },
    StreamingServicePreset { id: "streamway", display_name: "Streamway", protocol: StreamProtocol::Rtmp, default_server: "injest.streamway.in/LiveApp", stream_key_link: Some("https://app.streamway.in/broadcasts") },
    StreamingServicePreset { id: "shareplay-tv", display_name: "SharePlay.tv", protocol: StreamProtocol::Rtmp, default_server: "stream.shareplay.tv", stream_key_link: Some("https://playstudio.shareplay.tv/stream/settings") },
    StreamingServicePreset { id: "sheeta", display_name: "sheeta", protocol: StreamProtocol::Rtmp, default_server: "lsm.sheeta.com:1935/lsm", stream_key_link: None },
    StreamingServicePreset { id: "amazon-ivs", display_name: "Amazon IVS", protocol: StreamProtocol::Rtmps, default_server: "hkg06.contribute.live-video.net/app", stream_key_link: None },
    StreamingServicePreset { id: "dolby-optiview-real-time", display_name: "Dolby OptiView Real-time", protocol: StreamProtocol::Rtmps, default_server: "rtmp-auto.millicast.com:443/v2/pub", stream_key_link: Some("https://streaming.dolby.io") },
    StreamingServicePreset { id: "nfhs-network", display_name: "NFHS Network", protocol: StreamProtocol::Rtmp, default_server: "video.nfhsnetwork.com/manual", stream_key_link: Some("https://console.nfhsnetwork.com/nfhs-events/") },
    StreamingServicePreset { id: "vrcdn-live", display_name: "VRCDN - Live", protocol: StreamProtocol::Rtmp, default_server: "ingest.vrcdn.live/live", stream_key_link: None },
    StreamingServicePreset { id: "soop-global", display_name: "SOOP Global", protocol: StreamProtocol::Rtmp, default_server: "global-stream.sooplive.com/app", stream_key_link: Some("https://www.sooplive.com/dashboard") },
    StreamingServicePreset { id: "sportify", display_name: "Sportify", protocol: StreamProtocol::Rtmp, default_server: "stream.homegroundapp.com/live", stream_key_link: None },
];

/// Resolves a persisted service ID or legacy display name to a bounded entry.
#[must_use]
pub fn streaming_service_preset(value: &str) -> Option<StreamingServicePreset> {
    RTMP_SERVICE_PRESETS
        .iter()
        .copied()
        .find(|preset| preset.matches(value))
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WhipConfig {
    pub endpoint: String,
    pub bearer_token: Option<SecretString>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HlsConfig {
    pub directory: PathBuf,
    pub segment_duration_secs: u32,
    pub playlist_size: u32,
    pub low_latency: bool,
}

impl Default for HlsConfig {
    fn default() -> Self {
        Self {
            directory: PathBuf::from("hls"),
            segment_duration_secs: 4,
            playlist_size: 6,
            low_latency: false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RistConfig {
    pub host: String,
    pub port: u16,
    pub sender_buffer_ms: u32,
    pub shared_secret: Option<SecretString>,
}

impl Default for RistConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_owned(),
            port: 5_000,
            sender_buffer_ms: 1_000,
            shared_secret: None,
        }
    }
}

impl RistConfig {
    #[must_use]
    pub fn endpoint(&self) -> Option<String> {
        let host = self.host.trim();
        (!host.is_empty() && self.port > 0 && self.port.is_multiple_of(2))
            .then(|| format!("rist://{host}:{}", self.port))
    }
}

impl fmt::Debug for StreamTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StreamTarget")
            .field("protocol", &self.protocol())
            .field("endpoint", &"[REDACTED]")
            .finish()
    }
}

fn nonempty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SrtMode {
    #[default]
    Caller,
    Listener,
    Rendezvous,
}

impl SrtMode {
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Caller => "caller",
            Self::Listener => "listener",
            Self::Rendezvous => "rendezvous",
        }
    }

    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        match id.trim().to_ascii_lowercase().as_str() {
            "caller" => Some(Self::Caller),
            "listener" => Some(Self::Listener),
            "rendezvous" => Some(Self::Rendezvous),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u16)]
pub enum SrtKeyLength {
    Bits128 = 16,
    Bits192 = 24,
    Bits256 = 32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum VideoCodec {
    #[default]
    H264,
    Hevc,
    Av1,
    Vp8,
    ReferenceRle,
}

impl VideoCodec {
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::Hevc => "hevc",
            Self::Av1 => "av1",
            Self::Vp8 => "vp8",
            Self::ReferenceRle => "reference-rle",
        }
    }

    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        match id.trim().to_ascii_lowercase().as_str() {
            "h264" | "avc" => Some(Self::H264),
            "hevc" | "h265" => Some(Self::Hevc),
            "av1" => Some(Self::Av1),
            "vp8" => Some(Self::Vp8),
            "reference-rle" | "rle" => Some(Self::ReferenceRle),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AudioCodec {
    #[default]
    Aac,
    Opus,
    Pcm,
}

impl AudioCodec {
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Aac => "aac",
            Self::Opus => "opus",
            Self::Pcm => "pcm",
        }
    }

    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        match id.trim().to_ascii_lowercase().as_str() {
            "aac" | "avenc_aac" => Some(Self::Aac),
            "opus" | "opusenc" => Some(Self::Opus),
            "pcm" => Some(Self::Pcm),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EncoderImplementation(String);

impl EncoderImplementation {
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn is_automatic(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RateControl {
    #[default]
    Cbr,
    Vbr,
    Cqp,
}

impl RateControl {
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Cbr => "cbr",
            Self::Vbr => "vbr",
            Self::Cqp => "cqp",
        }
    }

    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        match id.trim().to_ascii_lowercase().as_str() {
            "cbr" => Some(Self::Cbr),
            "vbr" => Some(Self::Vbr),
            "cqp" | "quality" => Some(Self::Cqp),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EncoderPreset {
    Speed,
    #[default]
    Balanced,
    Quality,
}

impl EncoderPreset {
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Speed => "speed",
            Self::Balanced => "balanced",
            Self::Quality => "quality",
        }
    }

    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        match id.trim().to_ascii_lowercase().as_str() {
            "speed" | "fast" => Some(Self::Speed),
            "balanced" | "medium" => Some(Self::Balanced),
            "quality" | "slow" => Some(Self::Quality),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VideoEncoderConfig {
    pub codec: VideoCodec,
    pub implementation: EncoderImplementation,
    pub rate_control: RateControl,
    pub bitrate_kbps: u32,
    pub max_bitrate_kbps: Option<u32>,
    pub keyframe_interval_secs: u32,
    pub preset: EncoderPreset,
    pub profile: Option<String>,
    pub b_frames: u8,
}

impl Default for VideoEncoderConfig {
    fn default() -> Self {
        Self {
            codec: VideoCodec::H264,
            implementation: EncoderImplementation::default(),
            rate_control: RateControl::Cbr,
            bitrate_kbps: 6_000,
            max_bitrate_kbps: None,
            keyframe_interval_secs: 2,
            preset: EncoderPreset::Balanced,
            profile: Some("high".to_owned()),
            b_frames: 2,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioEncoderConfig {
    pub codec: AudioCodec,
    pub implementation: EncoderImplementation,
    pub bitrate_kbps: u32,
    pub sample_rate: u32,
    pub channels: u16,
    pub complexity: Option<u8>,
}

impl Default for AudioEncoderConfig {
    fn default() -> Self {
        Self {
            codec: AudioCodec::Aac,
            implementation: EncoderImplementation::default(),
            bitrate_kbps: 160,
            sample_rate: 48_000,
            channels: 2,
            complexity: None,
        }
    }
}

impl SrtKeyLength {
    #[must_use]
    pub const fn bytes(self) -> u16 {
        self as u16
    }

    #[must_use]
    pub const fn bits(self) -> u16 {
        self.bytes() * 8
    }

    #[must_use]
    pub const fn from_bytes(bytes: u16) -> Option<Self> {
        match bytes {
            16 => Some(Self::Bits128),
            24 => Some(Self::Bits192),
            32 => Some(Self::Bits256),
            _ => None,
        }
    }
}

/// Connection and encoder choices common to RTMP and RTMPS services.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RtmpConfig {
    pub service: String,
    pub server: String,
    pub stream_key: SecretString,
    pub video: VideoEncoderConfig,
    pub audio: AudioEncoderConfig,
    pub reconnect: bool,
    pub maximum_retries: u32,
    pub network_buffer_ms: u32,
}

impl Default for RtmpConfig {
    fn default() -> Self {
        Self {
            service: "Custom".to_owned(),
            server: "127.0.0.1/live".to_owned(),
            stream_key: SecretString::new("stream"),
            video: VideoEncoderConfig::default(),
            audio: AudioEncoderConfig::default(),
            reconnect: true,
            maximum_retries: 20,
            network_buffer_ms: 1_000,
        }
    }
}

impl RtmpConfig {
    /// Builds the transport endpoint while percent-encoding the secret path
    /// segment. The returned value is intended only for the connection API.
    #[must_use]
    pub fn endpoint(&self, protocol: StreamProtocol) -> Option<String> {
        let scheme = match protocol {
            StreamProtocol::Rtmp => "rtmp",
            StreamProtocol::Rtmps => "rtmps",
            _ => return None,
        };
        let server = self
            .server
            .trim()
            .trim_start_matches("rtmp://")
            .trim_start_matches("rtmps://")
            .trim_end_matches('/');
        let mut url = Url::parse(&format!("{scheme}://{server}")).ok()?;
        if !self.stream_key.is_empty() {
            url.path_segments_mut()
                .ok()?
                .push(self.stream_key.expose_secret());
        }
        Some(url.into())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SrtConfig {
    pub host: String,
    pub port: u16,
    pub mode: SrtMode,
    pub latency_ms: u32,
    pub passphrase: Option<SecretString>,
    pub pbkeylen: Option<SrtKeyLength>,
    pub stream_id: Option<String>,
    pub connect_timeout_ms: u32,
}

impl Default for SrtConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_owned(),
            port: 9_000,
            mode: SrtMode::Caller,
            latency_ms: 120,
            passphrase: None,
            pbkeylen: None,
            stream_id: None,
            connect_timeout_ms: 5_000,
        }
    }
}

impl SrtConfig {
    /// Builds an SRT URI using URL query encoding for all optional values.
    #[must_use]
    pub fn endpoint(&self) -> Option<String> {
        if self.host.trim().is_empty() || self.port == 0 {
            return None;
        }
        let mut url = Url::parse(&format!("srt://{}:{}", self.host.trim(), self.port)).ok()?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("mode", self.mode.id());
            query.append_pair("latency", &self.latency_ms.to_string());
            query.append_pair("connect_timeout", &self.connect_timeout_ms.to_string());
            if let Some(passphrase) = &self.passphrase {
                query.append_pair("passphrase", passphrase.expose_secret());
            }
            if let Some(key_length) = self.pbkeylen {
                query.append_pair("pbkeylen", &key_length.bytes().to_string());
            }
            if let Some(stream_id) = self.stream_id.as_deref() {
                query.append_pair("streamid", stream_id);
            }
        }
        Some(url.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_redacted_by_all_formatting_traits() {
        let secret = SecretString::new("do-not-print-this");
        assert_eq!(secret.to_string(), "[REDACTED]");
        assert!(!format!("{secret:?}").contains("do-not-print-this"));

        let config = RtmpConfig {
            stream_key: secret,
            ..RtmpConfig::default()
        };
        assert!(!format!("{config:?}").contains("do-not-print-this"));
        let target = StreamTarget::Rtmp(config);
        assert_eq!(
            format!("{target:?}"),
            "StreamTarget { protocol: Rtmp, endpoint: \"[REDACTED]\" }"
        );
    }

    #[test]
    fn protocol_and_srt_values_round_trip_through_stable_ids() {
        for protocol in [
            StreamProtocol::Rtmp,
            StreamProtocol::Rtmps,
            StreamProtocol::Srt,
            StreamProtocol::Whip,
            StreamProtocol::Hls,
            StreamProtocol::Rist,
            StreamProtocol::Reference,
        ] {
            assert_eq!(StreamProtocol::from_id(protocol.id()), Some(protocol));
        }
        for mode in [SrtMode::Caller, SrtMode::Listener, SrtMode::Rendezvous] {
            assert_eq!(SrtMode::from_id(mode.id()), Some(mode));
        }
    }

    #[test]
    fn extended_targets_are_typed_bounded_and_redacted() {
        let whip = StreamTarget::Whip(WhipConfig {
            endpoint: "https://service.example/whip".to_owned(),
            bearer_token: Some(SecretString::new("private-bearer")),
        });
        assert_eq!(whip.protocol(), StreamProtocol::Whip);
        assert!(!format!("{whip:?}").contains("private-bearer"));

        let hls = StreamTarget::Hls(HlsConfig::default());
        assert_eq!(hls.protocol(), StreamProtocol::Hls);
        assert_eq!(hls.endpoint().as_deref(), Some("hls"));

        let rist = StreamTarget::Rist(RistConfig::default());
        assert_eq!(rist.endpoint().as_deref(), Some("rist://127.0.0.1:5000"));
        let invalid = StreamTarget::Rist(RistConfig {
            port: 5_001,
            ..RistConfig::default()
        });
        assert!(invalid.endpoint().is_none());
    }

    #[test]
    fn endpoints_encode_secret_and_stream_identifier_components() {
        let rtmp = RtmpConfig {
            server: "media.example/live".to_owned(),
            stream_key: SecretString::new("key with/slash"),
            ..RtmpConfig::default()
        };
        assert_eq!(
            rtmp.endpoint(StreamProtocol::Rtmps).as_deref(),
            Some("rtmps://media.example/live/key%20with%2Fslash")
        );
        let srt = SrtConfig {
            passphrase: Some(SecretString::new("secret phrase")),
            stream_id: Some("#!::r=feed,m=publish".to_owned()),
            ..SrtConfig::default()
        };
        let endpoint = srt.endpoint().expect("valid SRT endpoint");
        assert!(endpoint.contains("passphrase=secret+phrase"));
        assert!(endpoint.contains("streamid=%23%21%3A%3Ar%3Dfeed%2Cm%3Dpublish"));
    }

    #[test]
    fn built_in_service_catalog_is_bounded_and_resolves_legacy_names() {
        assert_eq!(RTMP_SERVICE_PRESETS.len(), 82);
        assert_eq!(
            streaming_service_preset("Custom").map(StreamingServicePreset::id),
            Some("custom")
        );
        assert_eq!(
            streaming_service_preset("youtube-rtmps").map(StreamingServicePreset::protocol),
            Some(StreamProtocol::Rtmps)
        );
        assert_eq!(
            streaming_service_preset("Facebook Live")
                .and_then(StreamingServicePreset::stream_key_link),
            Some("https://www.facebook.com/live/producer?ref=OBS")
        );
        assert_eq!(
            streaming_service_preset("Twitch").map(StreamingServicePreset::default_server),
            Some("live-hkg.twitch.tv/app")
        );
        assert_eq!(
            streaming_service_preset("Amazon IVS").map(StreamingServicePreset::protocol),
            Some(StreamProtocol::Rtmps)
        );
        assert!(streaming_service_preset("not-a-service").is_none());
    }

    #[test]
    fn service_catalog_ids_and_endpoints_are_unique_and_typed() {
        for (index, preset) in RTMP_SERVICE_PRESETS.iter().enumerate() {
            assert!(
                RTMP_SERVICE_PRESETS[..index]
                    .iter()
                    .all(|other| other.id() != preset.id()),
                "duplicate service id: {}",
                preset.id()
            );
            assert!(!preset.default_server().is_empty());
            assert!(matches!(
                preset.protocol(),
                StreamProtocol::Rtmp | StreamProtocol::Rtmps
            ));
            let config = RtmpConfig {
                server: preset.default_server().to_owned(),
                ..RtmpConfig::default()
            };
            assert!(
                config.endpoint(preset.protocol()).is_some(),
                "invalid endpoint for service: {}",
                preset.id()
            );
        }
    }
}
