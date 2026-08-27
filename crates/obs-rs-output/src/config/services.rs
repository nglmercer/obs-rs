//! Bounded built-in streaming service and ingest-server catalog.

use super::StreamProtocol;

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

/// One additional pinned ingest server for a built-in service.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StreamingServerPreset {
    display_name: &'static str,
    server: &'static str,
}

impl StreamingServerPreset {
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        self.display_name
    }

    #[must_use]
    pub const fn server(self) -> &'static str {
        self.server
    }
}

const TWITCH_ADDITIONAL_SERVERS: [StreamingServerPreset; 45] = [
    StreamingServerPreset {
        display_name: "Asia: Seoul, South Korea",
        server: "live-sel.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "Asia: Singapore",
        server: "live-sin.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "Asia: Taipei, Taiwan",
        server: "live-tpe.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "Asia: Tokyo, Japan",
        server: "live-tyo.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "Australia: Sydney",
        server: "live-syd.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "EU: Amsterdam, NL",
        server: "live-ams.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "EU: Berlin, DE",
        server: "live-ber.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "Europe: Copenhagen, DK",
        server: "live-cph.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "EU: Frankfurt, DE",
        server: "live-fra.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "EU: Helsinki, FI",
        server: "live-hel.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "EU: Lisbon, Portugal",
        server: "live-lis.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "EU: London, UK",
        server: "live-lhr.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "EU: Madrid, Spain",
        server: "live-mad.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "EU: Marseille, FR",
        server: "live-mrs.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "EU: Milan, Italy",
        server: "live-mil.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "EU: Norway, Oslo",
        server: "live-osl.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "EU: Paris, FR",
        server: "live-cdg.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "EU: Prague, CZ",
        server: "live-prg.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "EU: Stockholm, SE",
        server: "live-arn.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "EU: Vienna, Austria",
        server: "live-vie.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "EU: Warsaw, Poland",
        server: "live-waw.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "NA: Mexico City",
        server: "live-qro.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "NA: Quebec, Canada",
        server: "live-ymq.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "NA: Toronto, Canada",
        server: "live-yto.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "South America: Argentina",
        server: "live-eze.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "South America: Chile",
        server: "live-scl.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "South America: Lima, Peru",
        server: "live-lim.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "South America: Medellin, Colombia",
        server: "live-mde.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "South America: Rio de Janeiro, Brazil",
        server: "live-rio.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "South America: Sao Paulo, Brazil",
        server: "live-sao.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "US Central: Dallas, TX",
        server: "live-dfw.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "US Central: Denver, CO",
        server: "live-den.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "US Central: Houston, TX",
        server: "live-hou.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "US Central: Salt Lake City, UT",
        server: "live-slc.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "US East: Ashburn, VA",
        server: "live-iad.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "US East: Atlanta, GA",
        server: "live-atl.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "US East: Chicago",
        server: "live-ord.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "US East: Miami, FL",
        server: "live-mia.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "US East: New York, NY",
        server: "live-jfk.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "US West: Los Angeles, CA",
        server: "live-lax.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "US West: Phoenix, AZ",
        server: "live-phx.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "US West: Portland, Oregon",
        server: "live-pdx.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "US West: San Francisco, CA",
        server: "live-sfo.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "US West: San Jose, CA",
        server: "live-sjc.twitch.tv/app",
    },
    StreamingServerPreset {
        display_name: "US West: Seattle, WA",
        server: "live-sea.twitch.tv/app",
    },
];

const YOUTUBE_RTMPS_ADDITIONAL_SERVERS: [StreamingServerPreset; 1] = [StreamingServerPreset {
    display_name: "Backup YouTube ingest server",
    server: "b.rtmps.youtube.com:443/live2?backup=1",
}];

const LOOLA_ADDITIONAL_SERVERS: [StreamingServerPreset; 4] = [
    StreamingServerPreset {
        display_name: "EU Central: Germany",
        server: "rtmp-eu.loola.tv/push",
    },
    StreamingServerPreset {
        display_name: "South America: Brazil",
        server: "rtmp-sa.loola.tv/push",
    },
    StreamingServerPreset {
        display_name: "Asia/Pacific: Singapore",
        server: "rtmp-sg.loola.tv/push",
    },
    StreamingServerPreset {
        display_name: "Middle East: Bahrain",
        server: "rtmp-me.loola.tv/push",
    },
];

const RESTREAM_ADDITIONAL_SERVERS: [StreamingServerPreset; 20] = [
    StreamingServerPreset {
        display_name: "EU-West (London, GB)",
        server: "london.restream.io/live",
    },
    StreamingServerPreset {
        display_name: "EU-West (Amsterdam, NL)",
        server: "amsterdam.restream.io/live",
    },
    StreamingServerPreset {
        display_name: "EU-West (Paris, FR)",
        server: "paris.restream.io/live",
    },
    StreamingServerPreset {
        display_name: "EU-Central (Frankfurt, DE)",
        server: "frankfurt.restream.io/live",
    },
    StreamingServerPreset {
        display_name: "EU-South (Madrid, Spain)",
        server: "madrid.restream.io/live",
    },
    StreamingServerPreset {
        display_name: "Turkey (Istanbul)",
        server: "istanbul.restream.io/live",
    },
    StreamingServerPreset {
        display_name: "US-West (Seattle, WA)",
        server: "seattle.restream.io/live",
    },
    StreamingServerPreset {
        display_name: "US-West (San Jose, CA)",
        server: "sanjose.restream.io/live",
    },
    StreamingServerPreset {
        display_name: "US-Central (Dallas, TX)",
        server: "dallas.restream.io/live",
    },
    StreamingServerPreset {
        display_name: "US-East (Chicago, IL)",
        server: "chicago.restream.io/live",
    },
    StreamingServerPreset {
        display_name: "US-East (New York, NY)",
        server: "newyork.restream.io/live",
    },
    StreamingServerPreset {
        display_name: "US-East (Washington, DC)",
        server: "washington.restream.io/live",
    },
    StreamingServerPreset {
        display_name: "NA-East (Toronto, Canada)",
        server: "toronto.restream.io/live",
    },
    StreamingServerPreset {
        display_name: "SA (Saint Paul, Brazil)",
        server: "saopaulo.restream.io/live",
    },
    StreamingServerPreset {
        display_name: "India (Bangalore)",
        server: "bangalore.restream.io/live",
    },
    StreamingServerPreset {
        display_name: "Asia (Hong Kong)",
        server: "hongkong.restream.io/live",
    },
    StreamingServerPreset {
        display_name: "Asia (Singapore)",
        server: "singapore.restream.io/live",
    },
    StreamingServerPreset {
        display_name: "Asia (Seoul, South Korea)",
        server: "seoul.restream.io/live",
    },
    StreamingServerPreset {
        display_name: "Asia (Tokyo, Japan)",
        server: "tokyo.restream.io/live",
    },
    StreamingServerPreset {
        display_name: "Australia (Sydney)",
        server: "sydney.restream.io/live",
    },
];

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

    /// Returns additional pinned servers after the primary endpoint.
    ///
    /// The primary server remains in the preset itself so persisted/custom
    /// server text has one canonical owner. Only services with a bounded
    /// regional list expose additional choices here.
    #[must_use]
    pub fn additional_servers(self) -> &'static [StreamingServerPreset] {
        match self.id {
            "twitch" => &TWITCH_ADDITIONAL_SERVERS,
            "youtube-rtmps" => &YOUTUBE_RTMPS_ADDITIONAL_SERVERS,
            "loola-tv" => &LOOLA_ADDITIONAL_SERVERS,
            "restream" => &RESTREAM_ADDITIONAL_SERVERS,
            _ => &[],
        }
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
