use super::*;

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the legacy migration fixture stays readable as one document"
)]
fn version_one_scene_sources_migrate_to_registry_and_items() {
    let legacy = r#"
{
  "format": "obs-rs-project",
  "version": 1,
  "title": "Legacy project",
  "active_profile": "live",
  "profiles": [
    {
      "id": "live",
      "name": "Live",
      "video": {
        "width": 640,
        "height": 360,
        "frame_rate": { "numerator": 30, "denominator": 1 }
      },
      "scenes": [
        {
          "id": "preview",
          "name": "Preview",
          "sources": [
            {
              "id": "camera",
              "kind": "camera_capture",
              "name": "Camera",
              "settings": { "device_id": "camera-0" },
              "transform": {
                "scale_x_milli": 1000,
                "scale_y_milli": 1000,
                "translate_x": 12,
                "translate_y": 0,
                "flip_x": false,
                "flip_y": false,
                "opacity": 255,
                "crop_left": 0,
                "crop_top": 0,
                "crop_right": 0,
                "crop_bottom": 0
              },
              "filters": [
                {
                  "id": "brightness",
                  "name": "Brightness",
                  "kind": "brightness",
                  "category": "effect",
                  "enabled": true,
                  "settings": { "milli": "750" }
                }
              ],
              "visible": true,
              "locked": false
            }
          ]
        },
        {
          "id": "program",
          "name": "Program",
          "sources": [
            {
              "id": "camera",
              "kind": "camera_capture",
              "name": "Camera",
              "settings": { "device_id": "camera-0" },
              "transform": {
                "scale_x_milli": 1000,
                "scale_y_milli": 1000,
                "translate_x": 0,
                "translate_y": 30,
                "flip_x": false,
                "flip_y": false,
                "opacity": 255,
                "crop_left": 0,
                "crop_top": 0,
                "crop_right": 0,
                "crop_bottom": 0
              },
              "filters": [
                {
                  "id": "brightness",
                  "name": "Brightness",
                  "kind": "brightness",
                  "category": "effect",
                  "enabled": true,
                  "settings": { "milli": "750" }
                }
              ],
              "visible": true,
              "locked": true
            }
          ]
        }
      ]
    }
  ]
}
"#;

    let migrated = Project::parse(legacy).expect("legacy project migrates");
    let profile = migrated.profile("live").expect("profile");
    assert_eq!(
        profile.sources().count(),
        1,
        "identical legacy sources are shared"
    );
    assert_eq!(
        profile
            .scene("preview")
            .expect("preview")
            .item("camera")
            .expect("preview item")
            .source_id()
            .as_str(),
        "camera"
    );
    assert_eq!(
        profile
            .scene("program")
            .expect("program")
            .item("camera")
            .expect("program item")
            .transform()
            .translate_y(),
        30
    );
    assert!(profile
        .scene("program")
        .expect("program")
        .item("camera")
        .expect("program item")
        .locked());
    assert_eq!(profile.source("camera").expect("source").filters().len(), 1);

    let encoded = migrated.serialize();
    assert!(encoded.contains(r#""version": 7"#), "{encoded}");
    assert!(encoded.contains(r#""items""#), "{encoded}");
    assert_eq!(
        Project::parse(&encoded).expect("new format parses"),
        migrated
    );
}
