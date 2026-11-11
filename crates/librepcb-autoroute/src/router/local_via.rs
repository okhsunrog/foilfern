//! Exact local connection with one through via and two short 45° leads.

use super::*;

const SEARCH_PITCH: i64 = 50_000;
const MAX_RADIUS: i64 = 1_500_000;
const MAX_LEAD: f64 = 4_000_000.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalViaSite {
    Exact(Point),
    /// Sample a 0.05 mm grid within this radius of the first anchor.
    Search(PositiveLength),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalViaBridge {
    pub position: Point,
    /// One optional 45° bend from each anchor to the via.
    pub elbows: [Option<Point>; 2],
    pub width: PositiveLength,
}

/// Checks short leads from two existing same-net anchors on different layers.
/// The caller validates the anchors against its board before invoking this.
pub fn find_local_via(
    problem: &RoutingProblem,
    net: NetId,
    anchors: [(Point, Layer); 2],
    site: LocalViaSite,
) -> Result<Option<LocalViaBridge>> {
    let router = Router::new(problem)?;
    router.local_via(net, anchors, site)
}

impl Router<'_> {
    fn local_via(
        &self,
        net: NetId,
        [(from, from_layer), (to, to_layer)]: [(Point, Layer); 2],
        site: LocalViaSite,
    ) -> Result<Option<LocalViaBridge>> {
        let layer = |layer| {
            self.routing
                .iter()
                .position(|&i| self.problem.copper_layers[i] == layer)
                .ok_or(Error::InvalidLocalVia)
        };
        let net = self
            .nets
            .binary_search(&net)
            .map_err(|_| Error::InvalidLocalVia)? as u32;
        let layers = [layer(from_layer)?, layer(to_layer)?];
        if layers[0] == layers[1] {
            return Err(Error::InvalidLocalVia);
        }
        let anchors = [P::from(from), P::from(to)];
        let width = self.width_of(net);
        let h = width.to_nm() / 2;
        let sites = match site {
            LocalViaSite::Exact(point) => vec![P::from(point)],
            LocalViaSite::Search(radius) => local_sites(anchors[0], radius)?,
        };
        let mut ranked: Vec<_> = sites
            .into_iter()
            .filter_map(|p| {
                let lengths = anchors.map(|anchor| octilinear_length(anchor, p));
                (lengths.iter().all(|&len| len > 0.0 && len <= MAX_LEAD))
                    .then_some((lengths[0] + lengths[1], p))
            })
            .collect();
        ranked.sort_unstable_by(|a, b| {
            a.0.total_cmp(&b.0)
                .then_with(|| (a.1.x, a.1.y).cmp(&(b.1.x, b.1.y)))
        });
        for (_, p) in ranked {
            let via_radius = self.rules.via_radius.ceil() as i64;
            if !self.within_region(p, via_radius) || !self.local_via_clear(p, net) {
                continue;
            }
            let Some(first) = self.local_via_lead(anchors[0], p, layers[0], net, h) else {
                continue;
            };
            let Some(second) = self.local_via_lead(anchors[1], p, layers[1], net, h) else {
                continue;
            };
            return Ok(Some(LocalViaBridge {
                position: p.to_point(),
                elbows: [first.map(P::to_point), second.map(P::to_point)],
                width,
            }));
        }
        Ok(None)
    }

    fn within_region(&self, p: P, half_width: i64) -> bool {
        self.problem
            .region
            .is_none_or(|r| r.contains(p.to_point(), Length::new(half_width + 1)))
    }

    fn local_via_clear(&self, p: P, net: u32) -> bool {
        let terminal_start =
            self.edge_count + self.problem.obstacles.len() + self.problem.holes.len();
        self.items
            .check_via_with_own_keep(p, net, None, &mut |_| false, |index, item| {
                // The annulus may touch same-net pad copper. Keep the drill
                // outside that copper and retain the ordinary rules for holes
                // and foreign copper. Board DRC checks the resulting patch.
                if let Some(t) = index.checked_sub(terminal_start)
                    && self.problem.pad_terminals.contains(&TerminalId(t))
                {
                    let (_, radius) = core_of(&self.problem.terminals[t].shape);
                    return Some(radius + self.rules.via_drill_radius);
                }
                item.via_keep_own
            })
            .is_ok()
    }

    fn local_via_lead(&self, a: P, b: P, layer: usize, net: u32, h: i64) -> Option<Option<P>> {
        if !self.within_region(a, h) || !self.within_region(b, h) {
            return None;
        }
        let clear = |a, b| self.check_segment_strict(a, b, layer, net, h);
        if is_octilinear(a, b) {
            return clear(a, b).then_some(None);
        }
        escape_elbows(a, b)
            .into_iter()
            .find(|&e| self.within_region(e, h) && clear(a, e) && clear(e, b))
            .map(Some)
    }
}

fn octilinear_length(a: P, b: P) -> f64 {
    let dx = a.x.abs_diff(b.x) as f64;
    let dy = a.y.abs_diff(b.y) as f64;
    dx.max(dy) + (std::f64::consts::SQRT_2 - 1.0) * dx.min(dy)
}

fn local_sites(center: P, radius: PositiveLength) -> Result<Vec<P>> {
    let r = radius.to_nm();
    if r > MAX_RADIUS {
        return Err(Error::InvalidLocalVia);
    }
    let start = |v: i64| v.saturating_sub(r).div_euclid(SEARCH_PITCH);
    let end = |v: i64| v.saturating_add(r).div_euclid(SEARCH_PITCH);
    let mut sites = Vec::new();
    for x in start(center.x)..=end(center.x) {
        for y in start(center.y)..=end(center.y) {
            let p = P::new(
                x.saturating_mul(SEARCH_PITCH),
                y.saturating_mul(SEARCH_PITCH),
            );
            if (p.x.abs_diff(center.x) as f64).hypot(p.y.abs_diff(center.y) as f64) <= r as f64 {
                sites.push(p);
            }
        }
    }
    Ok(sites)
}
