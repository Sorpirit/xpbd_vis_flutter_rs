use flutter_rust_bridge::frb;

// ---------------------------------------------------------------------------
// Types that cross the bridge (public fields, copied to Dart as plain classes)
// ---------------------------------------------------------------------------

pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

/// Everything the UI wants from the simulation this frame.
pub struct SimInput {
    pub gravity: f32,
    pub compliance: f32, // added to every distance constraint's own compliance
    pub damping: f32,
    pub substeps: u32,
    pub paused: bool,
    pub reset: bool,
    pub drag: Option<Vec2>, // cursor in metres while the mouse is held
    pub fire: Option<Vec2>, // launch the next slime at this point
    pub step_once: bool,    // while paused: run exactly one physics step
    pub debug: bool,        // also return velocities for the gizmos
}

pub struct SimStats {
    pub particles: u32,
    pub constraints: u32,
    pub contacts: u32, // particle-particle contacts in the last substep
    pub kinetic: f32,
    pub potential: f32,
    pub max_stretch: f32, // 0.02 means 2 % longer than rest length
}

/// Everything the UI needs to draw one frame.
pub struct SimFrame {
    pub positions: Vec<f32>,  // interpolated between the last two physics steps
    pub velocities: Vec<f32>, // empty unless `debug` was set
    pub contacts: Vec<u32>,   // pairs of touching particles in the last substep
    pub stats: SimStats,
    pub steps: u32, // fixed physics steps run by this call
}

pub enum BodyKind {
    Rope,
    Cloth,
    Soft,  // area-preserving ring
    Stone, // rigid
    Metal, // rigid, heavy
    Wood,  // rigid, light
}

/// One object in the scene: a contiguous range of particles.
pub struct BodyInfo {
    pub kind: BodyKind,
    pub start: u32,
    pub count: u32,
    pub outline: Vec<u32>, // particle indices of a polygon to fill; may be empty
    pub cols: u32,         // grid size for cloth and blocks, 0 otherwise
    pub rows: u32,
}

/// Static description of the scene, fetched once so Dart knows what to draw.
pub struct SceneInfo {
    pub bodies: Vec<BodyInfo>,
    pub lines: Vec<u32>,       // pairs of particle indices to draw as segments
    pub constraints: Vec<u32>, // every distance constraint, as index pairs
    pub rests: Vec<f32>,       // rest length of each of those
    pub ties: Vec<u32>,        // distance constraints that join two bodies
    pub radius: f32,           // every particle is a disc of this radius
    pub floor_y: f32,
    pub wall_x: f32, // walls at -wall_x and +wall_x
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

const FIXED_DT: f32 = 1.0 / 60.0; // physics always advances in steps of this size
const MAX_CATCH_UP: f32 = 0.25; // drop time beyond this instead of spiralling

const RADIUS: f32 = 0.05;
const FLOOR_Y: f32 = 5.0;
const WALL_X: f32 = 6.0;
const FLOOR_FRICTION: f32 = 0.8; // Coulomb coefficient of the floor
const CONTACT_FRICTION: f32 = 0.5; // share of sliding removed between touching particles
const GRAB_RADIUS: f32 = 0.3;
const DRAG_COMPLIANCE: f32 = 1e-4; // the mouse pulls through a spring this soft
const DRAG_MAX_ACCEL: f32 = 400.0; // m/s^2; caps the pull relative to the particle's mass
const MAX_MOVE: f32 = RADIUS; // furthest a particle may travel in one substep
const LAUNCH: (f32, f32) = (-5.0, 3.4);
const FLIGHT_TIME: f32 = 0.9; // seconds a fired slime takes to reach its target

// Uniform grid for finding nearby particles. One cell is one particle diameter.
const CELL: f32 = RADIUS * 2.0;
const GRID_TOP: f32 = -2.0;
const GRID_COLS: usize = 120; // 2 * WALL_X / CELL
const GRID_ROWS: usize = 70; // (FLOOR_Y - GRID_TOP) / CELL

/// Keep particles `a` and `b` at distance `rest`.
struct Distance {
    a: usize,
    b: usize,
    rest: f32,
    compliance: f32,
}

/// Keep the area enclosed by a closed ring of particles constant (2D "volume").
struct Area {
    start: usize,
    count: usize,
    rest: f32,
}

/// Keep a group of particles in its original shape (shape matching).
struct Rigid {
    start: usize,
    count: usize,
    rest: Vec<f32>, // rest positions, x0, y0, x1, y1, ...
}

struct Body {
    kind: BodyKind,
    start: usize,
    count: usize,
    outline: Vec<u32>,
    cols: usize,
    rows: usize,
}

/// A slime that can be fired: its particle range and radius.
struct Ammo {
    start: usize,
    count: usize,
    radius: f32,
}

#[frb(opaque)]
pub struct Sim {
    pos: Vec<f32>,  // x0, y0, x1, y1, ...
    prev: Vec<f32>, // position before the current substep
    last: Vec<f32>, // position before the current fixed step, for interpolation
    vel: Vec<f32>,
    inv_mass: Vec<f32>, // 0 = pinned
    body_of: Vec<u32>,  // which body each particle belongs to

    distances: Vec<Distance>,
    areas: Vec<Area>,
    rigids: Vec<Rigid>,
    bodies: Vec<Body>,
    lines: Vec<u32>,
    ties: Vec<u32>,
    ammo: Vec<Ammo>,
    next_ammo: usize,

    compliance: f32,
    gravity: f32,
    damping: f32,            // 1/s, velocity lost per second
    grabbed: Option<usize>,  // particle the mouse is pulling
    drag_from: (f32, f32),   // mouse target at the previous physics step
    drag_target: (f32, f32), // mouse target now
    accumulator: f32,        // real time not yet simulated
    contact_pairs: Vec<u32>,

    // scratch buffers, kept to avoid allocating every substep
    cell_of: Vec<u32>,
    cell_start: Vec<u32>,
    cell_items: Vec<u32>,
    grads: Vec<f32>,
}

impl Sim {
    /// The demo scene: a castle under siege. Units are metres, y points down.
    #[frb(sync)]
    pub fn scene() -> Sim {
        let mut sim = Sim {
            pos: Vec::new(),
            prev: Vec::new(),
            last: Vec::new(),
            vel: Vec::new(),
            inv_mass: Vec::new(),
            body_of: Vec::new(),
            distances: Vec::new(),
            areas: Vec::new(),
            rigids: Vec::new(),
            bodies: Vec::new(),
            lines: Vec::new(),
            ties: Vec::new(),
            ammo: Vec::new(),
            next_ammo: 0,
            compliance: 0.0,
            gravity: 9.81,
            damping: 0.0,
            grabbed: None,
            drag_from: (0.0, 0.0),
            drag_target: (0.0, 0.0),
            accumulator: 0.0,
            contact_pairs: Vec::new(),
            cell_of: Vec::new(),
            cell_start: vec![0; GRID_COLS * GRID_ROWS + 1],
            cell_items: Vec::new(),
            grads: Vec::new(),
        };

        // Two towers with a curtain wall between them, laid in running bond.
        sim.add_masonry(0.65, 40, 0, 3); // base, full width
        sim.add_masonry(0.65, 8, 3, 6); // left tower
        sim.add_masonry(3.85, 8, 3, 6); // right tower
                                        // Battlements: small blocks along the tops.
        let merlons = [0.75, 1.15, 3.95, 4.35].map(|x| sim.add_block(x, 6, 2, 2));
        for x in [1.65, 2.25, 2.85, 3.45] {
            sim.add_block(x, 3, 2, 2);
        }
        // A flag on each tower. The pole is a rigid body standing between the
        // two battlement blocks, and the flag is tied to it.
        for (x, left, right) in [
            (0.95, merlons[0], merlons[1]),
            (4.15, merlons[2], merlons[3]),
        ] {
            const ROWS: usize = 19;
            let pole = sim.add_pole(x, 6, ROWS);
            // Brace the foot of the pole to the block on either side, with
            // straight and diagonal links, so the three move as one unit.
            let foot = pole + (ROWS - 2) * 2; // first particle of the last two rows
            for (a, b) in [(2, 3), (0, 1), (2, 1), (0, 3)] {
                sim.add_distance(foot + a, left + b, 0.0, false);
            }
            for (a, b) in [(3, 2), (1, 0), (3, 0), (1, 2)] {
                sim.add_distance(foot + a, right + b, 0.0, false);
            }
            sim.add_flag(pole, 8, 5);
        }

        // A battering ram: a heavy block on a rope, pulled back and let go.
        let (rope_end, ex, ey) = sim.add_rope(-0.9, 1.3, 22, 0.1, 48.0);
        let ram = sim.add_box(ex - 0.15, ey + 0.1, 4, 4, BodyKind::Metal, 0.3);
        sim.add_distance(rope_end, ram + 1, 0.0, false); // top edge of the block
        sim.ties.push(rope_end as u32);
        sim.ties.push((ram + 1) as u32);

        // Slimes waiting on the left; double-click fires the next one.
        for x in [-5.5, -4.8, -4.1] {
            sim.add_blob(x, 4.6, 0.3, 14);
        }

        sim.prev = sim.pos.clone();
        sim.last = sim.pos.clone();
        sim.vel = vec![0.0; sim.pos.len()];
        sim.cell_of = vec![0; sim.inv_mass.len()];
        sim.cell_items = vec![0; sim.inv_mass.len()];
        sim
    }

    /// What the scene is made of. Call once; it does not change between frames.
    #[frb(sync)]
    pub fn info(&self) -> SceneInfo {
        SceneInfo {
            bodies: self
                .bodies
                .iter()
                .map(|b| BodyInfo {
                    kind: match b.kind {
                        BodyKind::Rope => BodyKind::Rope,
                        BodyKind::Cloth => BodyKind::Cloth,
                        BodyKind::Soft => BodyKind::Soft,
                        BodyKind::Stone => BodyKind::Stone,
                        BodyKind::Metal => BodyKind::Metal,
                        BodyKind::Wood => BodyKind::Wood,
                    },
                    start: b.start as u32,
                    count: b.count as u32,
                    outline: b.outline.clone(),
                    cols: b.cols as u32,
                    rows: b.rows as u32,
                })
                .collect(),
            lines: self.lines.clone(),
            constraints: self
                .distances
                .iter()
                .flat_map(|c| [c.a as u32, c.b as u32])
                .collect(),
            rests: self.distances.iter().map(|c| c.rest).collect(),
            ties: self.ties.clone(),
            radius: RADIUS,
            floor_y: FLOOR_Y,
            wall_x: WALL_X,
        }
    }

    /// Advance by `frame_dt` seconds of real time and return what to draw.
    ///
    /// Not `#[frb(sync)]`: Dart gets a Future and this runs on the bridge's
    /// worker thread pool, off the UI thread.
    pub fn advance(&mut self, frame_dt: f32, input: SimInput) -> SimFrame {
        if input.reset {
            *self = Sim::scene();
        }
        self.gravity = input.gravity;
        self.compliance = input.compliance;
        self.damping = input.damping;
        match input.drag {
            Some(p) if self.grabbed.is_some() => self.drag_target = (p.x, p.y),
            Some(p) => self.grab(p.x, p.y),
            None => self.grabbed = None,
        }
        if let Some(target) = input.fire {
            self.fire(target.x, target.y);
        }

        let mut steps = 0;
        if input.paused {
            if input.step_once {
                self.step(FIXED_DT, input.substeps.max(1));
                self.last.copy_from_slice(&self.pos); // show the new state exactly
                steps = 1;
            }
        } else {
            self.accumulator = (self.accumulator + frame_dt).min(MAX_CATCH_UP);
            while self.accumulator >= FIXED_DT {
                self.last.copy_from_slice(&self.pos);
                self.step(FIXED_DT, input.substeps.max(1));
                self.accumulator -= FIXED_DT;
                steps += 1;
            }
        }

        // The leftover time says how far we are between two physics states.
        let alpha = self.accumulator / FIXED_DT;
        let positions = self
            .last
            .iter()
            .zip(&self.pos)
            .map(|(a, b)| a + (b - a) * alpha)
            .collect();
        SimFrame {
            positions,
            velocities: if input.debug {
                self.vel.clone()
            } else {
                Vec::new()
            },
            contacts: self.contact_pairs.clone(),
            stats: self.stats(),
            steps,
        }
    }

    // -- scene building -----------------------------------------------------

    fn add_particle(&mut self, x: f32, y: f32, inv_mass: f32) -> usize {
        self.pos.push(x);
        self.pos.push(y);
        self.inv_mass.push(inv_mass);
        self.body_of.push(self.bodies.len() as u32);
        self.inv_mass.len() - 1
    }

    fn add_distance(&mut self, a: usize, b: usize, compliance: f32, draw: bool) {
        let dx = self.pos[a * 2] - self.pos[b * 2];
        let dy = self.pos[a * 2 + 1] - self.pos[b * 2 + 1];
        self.distances.push(Distance {
            a,
            b,
            rest: (dx * dx + dy * dy).sqrt(),
            compliance,
        });
        if draw {
            self.lines.push(a as u32);
            self.lines.push(b as u32);
        }
    }

    /// A chain of particles hanging from (x, y), pinned at its first particle
    /// and pulled back `degrees` to the left of straight down.
    /// Returns the index and position of its last particle.
    fn add_rope(
        &mut self,
        x: f32,
        y: f32,
        count: usize,
        spacing: f32,
        degrees: f32,
    ) -> (usize, f32, f32) {
        let start = self.inv_mass.len();
        let (sin, cos) = degrees.to_radians().sin_cos();
        for i in 0..count {
            let w = if i == 0 { 0.0 } else { 1.0 };
            let d = i as f32 * spacing;
            self.add_particle(x - d * sin, y + d * cos, w);
        }
        for i in 0..count - 1 {
            self.add_distance(start + i, start + i + 1, 0.0, true);
        }
        self.bodies.push(Body {
            kind: BodyKind::Rope,
            start,
            count,
            outline: Vec::new(),
            cols: 0,
            rows: 0,
        });
        let end = start + count - 1;
        (end, self.pos[end * 2], self.pos[end * 2 + 1])
    }

    /// A wooden pole two particles wide and `rows` tall, standing on course
    /// `course - 1`. Returns the index of its first (top-left) particle.
    fn add_pole(&mut self, x: f32, course: usize, rows: usize) -> usize {
        let spacing = RADIUS * 2.0;
        let bottom = Self::course_bottom(course);
        self.add_box(
            x,
            bottom - (rows - 1) as f32 * spacing,
            2,
            rows,
            BodyKind::Wood,
            0.25, // heavy enough that the flag's weight cannot tip it over
        )
    }

    /// A light cloth hanging from the top of the pole that starts at particle
    /// `pole`. Each particle of its left column is tied to the pole particle
    /// beside it, so the flag pulls on the pole and the pole on the flag.
    fn add_flag(&mut self, pole: usize, cols: usize, rows: usize) {
        let start = self.inv_mass.len();
        let spacing = RADIUS * 2.0;
        // the pole's right-hand column is at index pole + 2 * row + 1
        let x = self.pos[(pole + 1) * 2] + spacing;
        let y = self.pos[(pole + 1) * 2 + 1];
        for r in 0..rows {
            for c in 0..cols {
                self.add_particle(x + c as f32 * spacing, y + r as f32 * spacing, 10.0);
            }
            self.add_distance(start + r * cols, pole + 2 * r + 1, 0.0, false);
        }
        let at = |r: usize, c: usize| start + r * cols + c;
        for r in 0..rows {
            for c in 0..cols {
                if c + 1 < cols {
                    self.add_distance(at(r, c), at(r, c + 1), 0.0, true);
                }
                if r + 1 < rows {
                    self.add_distance(at(r, c), at(r + 1, c), 0.0, true);
                }
                // Diagonals resist shearing. Stiff ones would brace the grid
                // into a rigid plate in 2D; with none, the flag collapses into
                // a line. Very soft ones let it droop but keep its area.
                if c + 1 < cols && r + 1 < rows {
                    self.add_distance(at(r, c), at(r + 1, c + 1), 0.03, false);
                    self.add_distance(at(r, c + 1), at(r + 1, c), 0.03, false);
                }
            }
        }
        self.bodies.push(Body {
            kind: BodyKind::Cloth,
            start,
            count: rows * cols,
            outline: Vec::new(),
            cols,
            rows,
        });
    }

    /// A soft body: a ring of particles with stretchy edges whose enclosed
    /// area is conserved, like a water balloon. Every blob is also ammunition.
    fn add_blob(&mut self, cx: f32, cy: f32, radius: f32, count: usize) {
        let start = self.inv_mass.len();
        for i in 0..count {
            let a = i as f32 / count as f32 * std::f32::consts::TAU;
            self.add_particle(cx + radius * a.cos(), cy + radius * a.sin(), 1.0);
        }
        for i in 0..count {
            self.add_distance(start + i, start + (i + 1) % count, 2e-4, true);
        }
        let rest = self.ring_area(start, count);
        self.areas.push(Area { start, count, rest });
        self.ammo.push(Ammo {
            start,
            count,
            radius,
        });
        self.bodies.push(Body {
            kind: BodyKind::Soft,
            start,
            count,
            outline: (start..start + count).map(|i| i as u32).collect(),
            cols: 0,
            rows: 0,
        });
    }

    /// A rigid body: a block of particles held in shape by shape matching.
    /// (x, y) is the centre of its top-left particle. Returns its first index.
    fn add_box(
        &mut self,
        x: f32,
        y: f32,
        cols: usize,
        rows: usize,
        kind: BodyKind,
        inv_mass: f32,
    ) -> usize {
        let start = self.inv_mass.len();
        let spacing = RADIUS * 2.0;
        for r in 0..rows {
            for c in 0..cols {
                self.add_particle(x + c as f32 * spacing, y + r as f32 * spacing, inv_mass);
            }
        }
        let count = rows * cols;
        self.rigids.push(Rigid {
            start,
            count,
            rest: self.pos[start * 2..(start + count) * 2].to_vec(),
        });
        let corner = |r: usize, c: usize| (start + r * cols + c) as u32;
        self.bodies.push(Body {
            kind,
            start,
            count,
            outline: vec![
                corner(0, 0),
                corner(0, cols - 1),
                corner(rows - 1, cols - 1),
                corner(rows - 1, 0),
            ],
            cols,
            rows,
        });
        start
    }

    /// One stone block resting on course `course - 1` (0 is the floor).
    ///
    /// Discs stacked exactly on top of each other are unstable, like balancing
    /// balls. So each course is shifted half a particle sideways and lowered
    /// into the grooves of the one below, where it sits stably.
    /// Returns the index of its first particle.
    fn add_block(&mut self, x: f32, course: usize, cols: usize, rows: usize) -> usize {
        let spacing = RADIUS * 2.0;
        let bottom = Self::course_bottom(course);
        let shift = if course % 2 == 1 { RADIUS } else { 0.0 };
        self.add_box(
            x + shift,
            bottom - (rows - 1) as f32 * spacing,
            cols,
            rows,
            BodyKind::Stone,
            0.4,
        )
    }

    /// Height of the lowest row of particles in a course of masonry.
    fn course_bottom(course: usize) -> f32 {
        const COURSE_ROWS: usize = 3;
        let spacing = RADIUS * 2.0;
        let groove = spacing * 0.866; // row pitch of discs resting in grooves
        let pitch = (COURSE_ROWS - 1) as f32 * spacing + groove;
        FLOOR_Y - RADIUS - course as f32 * pitch
    }

    /// Courses `from..to` of a stone wall `width` particles wide, laid in
    /// running bond. Odd courses sit in the grooves of the course below, so
    /// they are one particle narrower, and they start and end with a short
    /// block so the joints do not line up.
    fn add_masonry(&mut self, x: f32, width: usize, from: usize, to: usize) {
        let spacing = RADIUS * 2.0;
        for k in from..to {
            let mut widths = Vec::new();
            if k % 2 == 0 {
                widths.resize(width / 4, 4);
            } else {
                let mut left = width - 1 - 2;
                widths.push(2);
                while left > 4 {
                    widths.push(4);
                    left -= 4;
                }
                // never end on a single particle: borrow one from the block before
                if left == 1 {
                    *widths.last_mut().unwrap() -= 1;
                    left = 2;
                }
                widths.push(left);
            }
            let mut c = 0;
            for w in widths {
                self.add_block(x + c as f32 * spacing, k, w, 3);
                c += w;
            }
        }
    }

    // -- solver -------------------------------------------------------------

    fn step(&mut self, dt: f32, substeps: u32) {
        let h = dt / substeps as f32;
        let n = self.inv_mass.len();
        for s in 0..substeps {
            // 1. predict: apply gravity, then move freely
            for i in 0..n {
                if self.inv_mass[i] == 0.0 {
                    continue;
                }
                let (x, y) = (i * 2, i * 2 + 1);
                self.vel[y] += self.gravity * h;
                self.prev[x] = self.pos[x];
                self.prev[y] = self.pos[y];
                let (dx, dy) = Self::limit(self.vel[x] * h, self.vel[y] * h);
                self.pos[x] += dx;
                self.pos[y] += dy;
            }

            // 2. solve: push positions until the constraints hold
            self.solve_mouse(h, (s + 1) as f32 / substeps as f32);
            self.solve_distances(h);
            self.solve_areas();
            self.solve_collisions();
            self.solve_rigids();
            self.solve_bounds();

            // 3. velocities from actual movement, with damping
            let keep = (1.0 - self.damping * h).max(0.0);
            for i in 0..n {
                let (x, y) = (i * 2, i * 2 + 1);
                // nothing may outrun the collision test
                let (dx, dy) = Self::limit(self.pos[x] - self.prev[x], self.pos[y] - self.prev[y]);
                self.pos[x] = self.prev[x] + dx;
                self.pos[y] = self.prev[y] + dy;
                self.vel[x] = dx / h * keep;
                self.vel[y] = dy / h * keep;
            }
        }
        self.drag_from = self.drag_target;
    }

    /// Shorten a substep's movement to at most MAX_MOVE. A particle that moves
    /// less than its own radius cannot jump past another one unnoticed.
    fn limit(dx: f32, dy: f32) -> (f32, f32) {
        let d2 = dx * dx + dy * dy;
        if d2 > MAX_MOVE * MAX_MOVE {
            let k = MAX_MOVE / d2.sqrt();
            (dx * k, dy * k)
        } else {
            (dx, dy)
        }
    }

    /// The mouse is a soft, force-limited spring: a distance constraint of rest
    /// length zero between the grabbed particle and the cursor. `t` runs from 0
    /// to 1 across the substeps so the target glides instead of jumping.
    fn solve_mouse(&mut self, h: f32, t: f32) {
        let Some(i) = self.grabbed else {
            return;
        };
        let w = self.inv_mass[i];
        if w == 0.0 {
            return;
        }
        let tx = self.drag_from.0 + (self.drag_target.0 - self.drag_from.0) * t;
        let ty = self.drag_from.1 + (self.drag_target.1 - self.drag_from.1) * t;
        let dx = self.pos[i * 2] - tx;
        let dy = self.pos[i * 2 + 1] - ty;
        let len = (dx * dx + dy * dy).sqrt();
        if len == 0.0 {
            return;
        }
        let alpha = DRAG_COMPLIANCE / (h * h);
        // lambda is force times h squared, so this caps the pull: a light
        // flag particle gets a gentle tug, a stone block a strong one
        let lambda = (-len / (w + alpha)).max(-DRAG_MAX_ACCEL / w * h * h);
        self.pos[i * 2] += w * lambda * dx / len;
        self.pos[i * 2 + 1] += w * lambda * dy / len;
    }

    fn solve_distances(&mut self, h: f32) {
        for k in 0..self.distances.len() {
            let c = &self.distances[k];
            let (a, b) = (c.a * 2, c.b * 2);
            let dx = self.pos[a] - self.pos[b];
            let dy = self.pos[a + 1] - self.pos[b + 1];
            let len = (dx * dx + dy * dy).sqrt();
            let (w1, w2) = (self.inv_mass[c.a], self.inv_mass[c.b]);
            let alpha = (c.compliance + self.compliance) / (h * h);
            let w = w1 + w2 + alpha;
            if len == 0.0 || w == 0.0 {
                continue;
            }
            let lambda = -(len - c.rest) / w;
            let (nx, ny) = (dx / len, dy / len);
            self.pos[a] += w1 * lambda * nx;
            self.pos[a + 1] += w1 * lambda * ny;
            self.pos[b] -= w2 * lambda * nx;
            self.pos[b + 1] -= w2 * lambda * ny;
        }
    }

    /// Signed area of a closed ring of particles (shoelace formula).
    fn ring_area(&self, start: usize, count: usize) -> f32 {
        let mut area = 0.0;
        for i in 0..count {
            let a = (start + i) * 2;
            let b = (start + (i + 1) % count) * 2;
            area += self.pos[a] * self.pos[b + 1] - self.pos[b] * self.pos[a + 1];
        }
        area * 0.5
    }

    /// Same recipe as the distance constraint, with C = area - rest area and
    /// one gradient per particle of the ring.
    fn solve_areas(&mut self) {
        for k in 0..self.areas.len() {
            let (start, count, rest) = {
                let a = &self.areas[k];
                (a.start, a.count, a.rest)
            };
            let c = self.ring_area(start, count) - rest;
            self.grads.clear();
            let mut denom = 0.0;
            for i in 0..count {
                let before = (start + (i + count - 1) % count) * 2;
                let after = (start + (i + 1) % count) * 2;
                let gx = 0.5 * (self.pos[after + 1] - self.pos[before + 1]);
                let gy = 0.5 * (self.pos[before] - self.pos[after]);
                self.grads.push(gx);
                self.grads.push(gy);
                denom += self.inv_mass[start + i] * (gx * gx + gy * gy);
            }
            if denom < 1e-9 {
                continue;
            }
            let lambda = -c / denom;
            for i in 0..count {
                let w = self.inv_mass[start + i];
                self.pos[(start + i) * 2] += w * lambda * self.grads[i * 2];
                self.pos[(start + i) * 2 + 1] += w * lambda * self.grads[i * 2 + 1];
            }
        }
    }

    fn cell_index(x: f32, y: f32) -> usize {
        let cx = (((x + WALL_X) / CELL) as i32).clamp(0, GRID_COLS as i32 - 1);
        let cy = (((y - GRID_TOP) / CELL) as i32).clamp(0, GRID_ROWS as i32 - 1);
        cy as usize * GRID_COLS + cx as usize
    }

    /// Push apart overlapping particles that belong to different bodies.
    /// Touching pairs are recorded in `contact_pairs`.
    fn solve_collisions(&mut self) {
        let n = self.inv_mass.len();
        self.contact_pairs.clear();

        // Sort particles into grid cells (counting sort).
        self.cell_start.fill(0);
        for i in 0..n {
            let c = Self::cell_index(self.pos[i * 2], self.pos[i * 2 + 1]);
            self.cell_of[i] = c as u32;
            self.cell_start[c + 1] += 1;
        }
        for c in 0..GRID_COLS * GRID_ROWS {
            self.cell_start[c + 1] += self.cell_start[c];
        }
        let mut fill = self.cell_start.clone();
        for i in 0..n {
            let c = self.cell_of[i] as usize;
            self.cell_items[fill[c] as usize] = i as u32;
            fill[c] += 1;
        }

        // Check each particle against the 3x3 block of cells around it.
        let min_dist = RADIUS * 2.0;
        for i in 0..n {
            let cell = self.cell_of[i] as usize;
            let (cx, cy) = ((cell % GRID_COLS) as i32, (cell / GRID_COLS) as i32);
            for ny in (cy - 1).max(0)..=(cy + 1).min(GRID_ROWS as i32 - 1) {
                for nx in (cx - 1).max(0)..=(cx + 1).min(GRID_COLS as i32 - 1) {
                    let c = ny as usize * GRID_COLS + nx as usize;
                    for k in self.cell_start[c]..self.cell_start[c + 1] {
                        let j = self.cell_items[k as usize] as usize;
                        if j <= i || self.body_of[i] == self.body_of[j] {
                            continue;
                        }
                        let dx = self.pos[j * 2] - self.pos[i * 2];
                        let dy = self.pos[j * 2 + 1] - self.pos[i * 2 + 1];
                        let d2 = dx * dx + dy * dy;
                        if d2 >= min_dist * min_dist || d2 == 0.0 {
                            continue;
                        }
                        let (w1, w2) = (self.inv_mass[i], self.inv_mass[j]);
                        if w1 + w2 == 0.0 {
                            continue;
                        }
                        let d = d2.sqrt();
                        let push = (min_dist - d) / (w1 + w2);
                        let (nx, ny) = (dx / d, dy / d);
                        self.pos[i * 2] -= nx * push * w1;
                        self.pos[i * 2 + 1] -= ny * push * w1;
                        self.pos[j * 2] += nx * push * w2;
                        self.pos[j * 2 + 1] += ny * push * w2;

                        // friction: undo part of how far the two slid past
                        // each other during this substep
                        let rx = (self.pos[j * 2] - self.prev[j * 2])
                            - (self.pos[i * 2] - self.prev[i * 2]);
                        let ry = (self.pos[j * 2 + 1] - self.prev[j * 2 + 1])
                            - (self.pos[i * 2 + 1] - self.prev[i * 2 + 1]);
                        let along = rx * nx + ry * ny;
                        let k = CONTACT_FRICTION / (w1 + w2);
                        let (sx, sy) = ((rx - along * nx) * k, (ry - along * ny) * k);
                        self.pos[i * 2] += sx * w1;
                        self.pos[i * 2 + 1] += sy * w1;
                        self.pos[j * 2] -= sx * w2;
                        self.pos[j * 2 + 1] -= sy * w2;

                        self.contact_pairs.push(i as u32);
                        self.contact_pairs.push(j as u32);
                    }
                }
            }
        }
    }

    /// Shape matching: find the rotation and translation of the rest shape
    /// that best fits the current particles, then snap the particles to it.
    fn solve_rigids(&mut self) {
        for k in 0..self.rigids.len() {
            let r = &self.rigids[k];
            // A held particle counts as very heavy, so the body pivots around it.
            let weight = |w: f32| if w == 0.0 { 1000.0 } else { 1.0 / w };

            let (mut m, mut cx, mut cy, mut qcx, mut qcy) = (0.0, 0.0, 0.0, 0.0, 0.0);
            for i in 0..r.count {
                let p = r.start + i;
                let wt = weight(self.inv_mass[p]);
                m += wt;
                cx += wt * self.pos[p * 2];
                cy += wt * self.pos[p * 2 + 1];
                qcx += wt * r.rest[i * 2];
                qcy += wt * r.rest[i * 2 + 1];
            }
            let (cx, cy, qcx, qcy) = (cx / m, cy / m, qcx / m, qcy / m);

            let (mut a, mut b) = (0.0, 0.0);
            for i in 0..r.count {
                let p = r.start + i;
                let wt = weight(self.inv_mass[p]);
                let (px, py) = (self.pos[p * 2] - cx, self.pos[p * 2 + 1] - cy);
                let (qx, qy) = (r.rest[i * 2] - qcx, r.rest[i * 2 + 1] - qcy);
                a += wt * (qx * px + qy * py);
                b += wt * (qx * py - qy * px);
            }
            let len = (a * a + b * b).sqrt();
            if len < 1e-9 {
                continue;
            }
            let (cos, sin) = (a / len, b / len);

            for i in 0..r.count {
                let p = r.start + i;
                if self.inv_mass[p] == 0.0 {
                    continue;
                }
                let (qx, qy) = (r.rest[i * 2] - qcx, r.rest[i * 2 + 1] - qcy);
                self.pos[p * 2] = cx + cos * qx - sin * qy;
                self.pos[p * 2 + 1] = cy + sin * qx + cos * qy;
            }
        }
    }

    /// Floor and walls.
    fn solve_bounds(&mut self) {
        for i in 0..self.inv_mass.len() {
            if self.inv_mass[i] == 0.0 {
                continue;
            }
            let (x, y) = (i * 2, i * 2 + 1);
            let sunk = self.pos[y] - (FLOOR_Y - RADIUS);
            if sunk > 0.0 {
                self.pos[y] = FLOOR_Y - RADIUS;
                // Coulomb friction: undo sideways movement, but by no more than
                // the coefficient times how hard the floor had to push back.
                // Light pressure lets things slide; a loaded base stays put.
                let grip = FLOOR_FRICTION * sunk;
                self.pos[x] -= (self.pos[x] - self.prev[x]).clamp(-grip, grip);
            }
            self.pos[x] = self.pos[x].clamp(-WALL_X + RADIUS, WALL_X - RADIUS);
        }
    }

    // -- mouse --------------------------------------------------------------

    /// Start pulling the movable particle nearest to (x, y), if one is close.
    fn grab(&mut self, x: f32, y: f32) {
        let mut best = None;
        let mut best_d = GRAB_RADIUS * GRAB_RADIUS;
        for i in 0..self.inv_mass.len() {
            if self.inv_mass[i] == 0.0 {
                continue;
            }
            let dx = self.pos[i * 2] - x;
            let dy = self.pos[i * 2 + 1] - y;
            let d = dx * dx + dy * dy;
            if d < best_d {
                best_d = d;
                best = Some(i);
            }
        }
        self.grabbed = best;
        self.drag_from = (x, y);
        self.drag_target = (x, y);
    }

    /// Put the next slime at the launch point and throw it so that it reaches
    /// (x, y) after FLIGHT_TIME seconds.
    fn fire(&mut self, x: f32, y: f32) {
        if self.ammo.is_empty() {
            return;
        }
        let (start, count, radius) = {
            let a = &self.ammo[self.next_ammo % self.ammo.len()];
            (a.start, a.count, a.radius)
        };
        self.next_ammo += 1;
        if self
            .grabbed
            .is_some_and(|g| g >= start && g < start + count)
        {
            self.grabbed = None;
        }
        let vx = (x - LAUNCH.0) / FLIGHT_TIME;
        let vy = (y - LAUNCH.1) / FLIGHT_TIME - 0.5 * self.gravity * FLIGHT_TIME;
        for i in 0..count {
            let a = i as f32 / count as f32 * std::f32::consts::TAU;
            let p = (start + i) * 2;
            self.pos[p] = LAUNCH.0 + radius * a.cos();
            self.pos[p + 1] = LAUNCH.1 + radius * a.sin();
            self.prev[p] = self.pos[p];
            self.prev[p + 1] = self.pos[p + 1];
            self.last[p] = self.pos[p];
            self.last[p + 1] = self.pos[p + 1];
            self.vel[p] = vx;
            self.vel[p + 1] = vy;
        }
    }

    // -- stats --------------------------------------------------------------

    fn stats(&self) -> SimStats {
        let n = self.inv_mass.len();
        let (mut kinetic, mut potential, mut max_stretch) = (0.0f32, 0.0f32, 0.0f32);
        for i in 0..n {
            let w = self.inv_mass[i];
            if w == 0.0 {
                continue;
            }
            let m = 1.0 / w;
            let (vx, vy) = (self.vel[i * 2], self.vel[i * 2 + 1]);
            kinetic += 0.5 * m * (vx * vx + vy * vy);
            // height above the floor; y points down
            potential += m * self.gravity * (FLOOR_Y - self.pos[i * 2 + 1]);
        }
        for c in &self.distances {
            let dx = self.pos[c.a * 2] - self.pos[c.b * 2];
            let dy = self.pos[c.a * 2 + 1] - self.pos[c.b * 2 + 1];
            let len = (dx * dx + dy * dy).sqrt();
            max_stretch = max_stretch.max((len / c.rest - 1.0).abs());
        }
        SimStats {
            particles: n as u32,
            constraints: (self.distances.len() + self.areas.len() + self.rigids.len()) as u32,
            contacts: (self.contact_pairs.len() / 2) as u32,
            kinetic,
            potential,
            max_stretch,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> SimInput {
        SimInput {
            gravity: 9.81,
            compliance: 0.0,
            damping: 0.0,
            substeps: 10,
            paused: false,
            reset: false,
            drag: None,
            fire: None,
            step_once: false,
            debug: false,
        }
    }

    #[test]
    fn scene_stays_finite_and_inside_the_room() {
        let mut sim = Sim::scene();
        for _ in 0..600 {
            sim.advance(FIXED_DT, input());
        }
        for i in 0..sim.inv_mass.len() {
            let (x, y) = (sim.pos[i * 2], sim.pos[i * 2 + 1]);
            assert!(x.is_finite() && y.is_finite(), "particle {i} is not finite");
            assert!(
                x.abs() <= WALL_X && y <= FLOOR_Y,
                "particle {i} left the room"
            );
        }
    }

    /// Writes snapshots for plotting when XPBD_DUMP names a directory.
    #[test]
    fn dump_snapshots() {
        let Ok(dir) = std::env::var("XPBD_DUMP") else {
            return;
        };
        let mut sim = Sim::scene();
        for frame in 0..=640 {
            if frame % 80 == 0 {
                let mut out = String::new();
                for i in 0..sim.inv_mass.len() {
                    out += &format!(
                        "{},{},{}\n",
                        sim.body_of[i],
                        sim.pos[i * 2],
                        sim.pos[i * 2 + 1]
                    );
                }
                std::fs::write(format!("{dir}/f{frame:04}.csv"), out).unwrap();
                let s = sim.stats();
                println!(
                    "t={:.1}s ke={:.1} pe={:.1} stretch={:.3} contacts={}",
                    frame as f32 / 60.0,
                    s.kinetic,
                    s.potential,
                    s.max_stretch,
                    s.contacts
                );
            }
            let mut i = input();
            if frame == 300 {
                i.fire = Some(Vec2 { x: 2.6, y: 3.6 });
            }
            if frame > 400 && frame <= 405 {
                i.drag = Some(Vec2 { x: 4.8, y: 1.6 }); // take hold of a flag
            } else if frame > 405 {
                // then yank it hard down and left, through the tower
                i.drag = Some(Vec2 { x: 1.0, y: 4.8 });
            }
            sim.advance(FIXED_DT, i);
        }
    }
}
