//! A project's or group's package registry and container registry.

use crate::hub::{Act, Hub, Req};
use crate::json::Json as _;
use crate::resource::{ListSpec, Row};
use crate::time;
use crate::widgets;
use gpui::{AnyElement, Context, IntoElement as _, ParentElement as _, Styled as _};

const TYPES: [(&str, &str); 9] = [
    ("", "All packages"),
    ("npm", "npm"),
    ("maven", "Maven"),
    ("pypi", "PyPI"),
    ("nuget", "NuGet"),
    ("generic", "Generic"),
    ("conan", "Conan"),
    ("helm", "Helm"),
    ("containers", "Containers"),
];

impl Hub {
    /// Packages under `base` (`/api/v4/projects/…` or `/api/v4/groups/…`),
    /// by type, and the container images there.
    pub fn gl_packages(&mut self, base: &str, cx: &mut Context<Self>) -> AnyElement {
        let type_key = format!("packages.type:{base}");
        let kind = self.choice(&type_key, "");
        let web = crate::forge::web();
        let list = if kind == "containers" {
            let path = format!("{base}/registry/repositories?tags_count=true");
            let inval = path.clone();
            let spec = ListSpec::new(path, move |r| {
                let project = r.i("project_id");
                Row::new(r.s("path"))
                    .icon("package", widgets::gray())
                    .meta(format!(
                        "{} tags  ·  created {}",
                        r.i("tags_count"),
                        time::ago(&r.s("created_at"))
                    ))
                    .danger(
                        "Delete",
                        Req::rest(
                            "DELETE",
                            format!(
                                "/api/v4/projects/{project}/registry/repositories/{}",
                                r.i("id")
                            ),
                        )
                        .ok("Container repository deleted")
                        .inval(inval.clone())
                        .act()
                        .confirm(
                            "Delete this container repository?",
                            "All of its tags are deleted.",
                            "Delete",
                        ),
                    )
            })
            .empty("No container images.");
            self.list(&spec, cx)
        } else {
            let mut path = format!("{base}/packages?order_by=created_at&sort=desc");
            if !kind.is_empty() {
                path.push_str(&format!("&package_type={kind}"));
            }
            let inval = format!("{base}/packages");
            let spec = ListSpec::new(path, move |p| {
                let project = p.i("project_id");
                let page = p.s("_links.web_path");
                let mut row = Row::new(p.s("name"))
                    .icon("package", widgets::gray())
                    .meta(format!(
                        "{}  ·  {}  ·  {}{}",
                        p.s("version"),
                        p.s("package_type"),
                        time::ago(&p.s("created_at")),
                        if p.has("project_path") {
                            format!("  ·  {}", p.s("project_path"))
                        } else {
                            String::new()
                        }
                    ));
                if !page.is_empty() {
                    row = row
                        .open(Act::Url(format!("{web}{page}")))
                        .action(crate::forge::open_on(), Act::Url(format!("{web}{page}")));
                }
                row.danger(
                    "Delete",
                    Req::rest(
                        "DELETE",
                        format!("/api/v4/projects/{project}/packages/{}", p.i("id")),
                    )
                    .ok("Package deleted")
                    .inval(inval.clone())
                    .act()
                    .confirm(
                        format!("Delete {} {}?", p.s("name"), p.s("version")),
                        "Its files are deleted.",
                        "Delete",
                    ),
                )
            })
            .empty("No packages of this kind.");
            self.list(&spec, cx)
        };
        widgets::col()
            .gap_3()
            .child(widgets::chips(
                TYPES
                    .iter()
                    .map(|(v, l)| (l.to_string(), kind == *v, Act::choose(&type_key, *v)))
                    .collect(),
            ))
            .child(list)
            .into_any_element()
    }
}
