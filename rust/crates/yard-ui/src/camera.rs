//! Orbit camera and the small amount of matrix math it needs.

/// Column-major 4x4, as WebGL expects.
pub type Mat4 = [f32; 16];

pub fn multiply(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [0.0; 16];
    for col in 0..4 {
        for row in 0..4 {
            out[col * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[col * 4 + k]).sum();
        }
    }
    out
}

fn perspective(fov_y: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    let f = 1.0 / (fov_y / 2.0).tan();
    let mut m = [0.0; 16];
    m[0] = f / aspect;
    m[5] = f;
    m[10] = (far + near) / (near - far);
    m[11] = -1.0;
    m[14] = 2.0 * far * near / (near - far);
    m
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalize(a: [f32; 3]) -> [f32; 3] {
    let len = dot(a, a).sqrt().max(1e-6);
    [a[0] / len, a[1] / len, a[2] / len]
}

fn look_at(eye: [f32; 3], target: [f32; 3], up: [f32; 3]) -> Mat4 {
    let f = normalize(sub(target, eye));
    let s = normalize(cross(f, up));
    let u = cross(s, f);
    [
        s[0], u[0], -f[0], 0.0,
        s[1], u[1], -f[1], 0.0,
        s[2], u[2], -f[2], 0.0,
        -dot(s, eye), -dot(u, eye), dot(f, eye), 1.0,
    ]
}

/// Vertical field of view in radians.
const FOV_Y: f32 = 0.9;

/// A camera that circles a point. Angles are radians.
///
/// StarMade's axes are right-handed, like WebGL's: +Y is up and +Z is the
/// direction a ship flies, which puts +X on the pilot's left. (The game's side
/// constant called RIGHT points to +X all the same; its block textures only
/// read correctly unmirrored, which is what settles it.) Design space is
/// therefore drawn as it is, with no mirroring.
#[derive(Clone, Copy, Debug)]
pub struct Camera {
    /// The point being circled.
    target: [f32; 3],
    distance: f32,
    yaw: f32,
    pitch: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self { target: [0.0; 3], distance: 40.0, yaw: 0.7, pitch: 0.45 }
    }
}

impl Camera {
    /// Points the camera at a design-space box and backs off to see all of it.
    pub fn frame(&mut self, min: [f32; 3], max: [f32; 3]) {
        self.target = [(min[0] + max[0]) / 2.0, (min[1] + max[1]) / 2.0, (min[2] + max[2]) / 2.0];
        let d = sub(max, min);
        // A little room around it; a design of a few blocks does not fill the view.
        self.distance = (dot(d, d).sqrt() * 1.3).max(14.0);
    }

    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.yaw -= dx * 0.008;
        self.pitch = (self.pitch + dy * 0.008).clamp(-1.5, 1.5);
    }

    /// Turns the camera where it stands, the way mouse look turns the head
    /// in the game: the point it circles swings round in front of it.
    pub fn look(&mut self, dx: f32, dy: f32) {
        let eye = self.eye();
        self.yaw -= dx * 0.003;
        self.pitch = (self.pitch + dy * 0.003).clamp(-1.5, 1.5);
        let away = sub(self.eye(), self.target);
        self.target = sub(eye, away);
    }

    pub fn zoom(&mut self, wheel_delta: f32) {
        self.distance = (self.distance * (wheel_delta * 0.0012).exp()).clamp(2.0, 20_000.0);
    }

    /// Slides the target across the view plane, following the pointer.
    pub fn pan(&mut self, dx: f32, dy: f32) {
        let forward = normalize(sub(self.target, self.eye()));
        let right = normalize(cross(forward, [0.0, 1.0, 0.0]));
        let up = cross(right, forward);
        let scale = self.distance * 0.0016;
        for i in 0..3 {
            self.target[i] += (-right[i] * dx + up[i] * dy) * scale;
        }
    }

    /// Moves the camera and the point it circles together along the camera's
    /// own axes, the way W, A, S, D, E and Q move the build camera in the game.
    /// `along` is (right, up, forward), each from -1 to 1.
    pub fn fly(&mut self, along: [f32; 3], seconds: f32, fast: bool) {
        let forward = normalize(sub(self.target, self.eye()));
        let right = normalize(cross(forward, [0.0, 1.0, 0.0]));
        let up = cross(right, forward);
        // The game moves 5 blocks a second, 25 with the fast key held. From far
        // out that would hardly show, so here the speed grows with the distance.
        let speed = (self.distance * 0.5).max(5.0) * if fast { 5.0 } else { 1.0 };
        for i in 0..3 {
            self.target[i] += (right[i] * along[0] + up[i] * along[1] + forward[i] * along[2]) * speed * seconds;
        }
    }

    /// Which way the camera looks and which way its right hand points, each
    /// as the nearest of the four horizontal design axes: how the movement
    /// keys push something along the grid as seen from here.
    pub fn heading(&self) -> ([i32; 3], [i32; 3]) {
        // The camera looks from (sin yaw, ., cos yaw) towards its target.
        let (x, z) = (-self.yaw.sin(), -self.yaw.cos());
        let forward = if x.abs() >= z.abs() { [x.signum() as i32, 0, 0] } else { [0, 0, z.signum() as i32] };
        // forward x up
        let right = [-forward[2], 0, forward[0]];
        (forward, right)
    }

    fn eye(&self) -> [f32; 3] {
        [
            self.target[0] + self.distance * self.pitch.cos() * self.yaw.sin(),
            self.target[1] + self.distance * self.pitch.sin(),
            self.target[2] + self.distance * self.pitch.cos() * self.yaw.cos(),
        ]
    }

    /// The ray under a point of the viewport, in design space: origin and
    /// direction. `x` and `y` run from -1 (left, bottom) to 1 (right, top).
    pub fn ray(&self, x: f32, y: f32, aspect: f32) -> ([f32; 3], [f32; 3]) {
        let eye = self.eye();
        let forward = normalize(sub(self.target, eye));
        let right = normalize(cross(forward, [0.0, 1.0, 0.0]));
        let up = cross(right, forward);
        let half = (FOV_Y / 2.0).tan();
        let mut dir = [0.0; 3];
        for i in 0..3 {
            dir[i] = forward[i] + right[i] * x * half * aspect + up[i] * y * half;
        }
        (eye, normalize(dir))
    }

    /// Model-view-projection matrix for design-space positions.
    pub fn matrix(&self, aspect: f32) -> Mat4 {
        let projection = perspective(FOV_Y, aspect, (self.distance * 0.01).max(0.1), self.distance * 4.0 + 2000.0);
        let view = look_at(self.eye(), self.target, [0.0, 1.0, 0.0]);
        multiply(&projection, &view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(m: &Mat4, p: [f32; 3]) -> [f32; 3] {
        let v = [p[0], p[1], p[2], 1.0];
        let mut out = [0.0; 4];
        for row in 0..4 {
            out[row] = (0..4).map(|k| m[k * 4 + row] * v[k]).sum();
        }
        [out[0] / out[3], out[1] / out[3], out[2] / out[3]]
    }

    /// Seen from behind (camera on the -Z side looking forward along the ship),
    /// +X is on the left of the screen, as in the game, and up is up.
    #[test]
    fn from_behind_plus_x_is_on_the_left_and_up_is_up() {
        let mut c = Camera::default();
        c.frame([-10.0, -10.0, -10.0], [10.0, 10.0, 10.0]);
        c.yaw = std::f32::consts::PI; // eye at -Z
        c.pitch = 0.0;
        let m = c.matrix(1.0);
        assert!(project(&m, [5.0, 0.0, 0.0])[0] < 0.0, "+X should be on the left");
        assert!(project(&m, [0.0, 5.0, 0.0])[1] > 0.0, "+Y should be up");
        // The nose (+Z) is farther from the camera than the stern.
        assert!(project(&m, [0.0, 0.0, 5.0])[2] > project(&m, [0.0, 0.0, -5.0])[2]);
    }

    /// The ray under a pixel must pass through whatever is drawn at that pixel.
    #[test]
    fn rays_go_through_the_points_they_are_cast_at() {
        let mut c = Camera::default();
        c.frame([-10.0, -4.0, -30.0], [10.0, 4.0, 30.0]);
        c.orbit(140.0, -60.0);
        let aspect = 1.6;
        let m = c.matrix(aspect);
        for p in [[7.0, 2.0, -11.0], [-9.0, -3.0, 25.0], [0.0, 0.0, 0.0]] {
            let screen = project(&m, p);
            let (origin, dir) = c.ray(screen[0], screen[1], aspect);
            // Distance from p to the ray.
            let to_p = sub(p, origin);
            let along = dot(to_p, dir);
            let closest = [origin[0] + dir[0] * along, origin[1] + dir[1] * along, origin[2] + dir[2] * along];
            let miss = sub(p, closest);
            assert!(along > 0.0, "point is behind the camera");
            assert!(dot(miss, miss).sqrt() < 1e-2, "ray misses {p:?} by {}", dot(miss, miss).sqrt());
        }
    }

    #[test]
    fn flying_moves_along_what_the_screen_shows() {
        let mut c = Camera::default();
        c.frame([-10.0, -10.0, -10.0], [10.0, 10.0, 10.0]);
        c.orbit(90.0, 30.0);
        let before = c.matrix(1.0);
        let start = c.target;
        // To the right: what was in the middle is now left of it.
        let mut right = c;
        right.fly([1.0, 0.0, 0.0], 1.0, false);
        assert!(project(&right.matrix(1.0), start)[0] < -0.01);
        // Up: the old middle drops below it.
        let mut up = c;
        up.fly([0.0, 1.0, 0.0], 1.0, false);
        assert!(project(&up.matrix(1.0), start)[1] < -0.01);
        // Forward: the camera goes where it was looking, so a point straight
        // ahead stays in the middle and comes closer.
        let mut ahead = c;
        ahead.fly([0.0, 0.0, 1.0], 1.0, false);
        let seen = project(&before, ahead.target);
        assert!(seen[0].abs() < 1e-3 && seen[1].abs() < 1e-3);
        assert!(seen[2] > project(&before, start)[2]);
        // The fast key covers five times the ground.
        let mut fast = c;
        fast.fly([0.0, 0.0, 1.0], 1.0, true);
        let ground = |cam: &Camera| dot(sub(cam.target, start), sub(cam.target, start)).sqrt();
        assert!((ground(&fast) / ground(&ahead) - 5.0).abs() < 1e-3);
    }

    #[test]
    fn looking_round_turns_the_camera_where_it_stands() {
        let mut c = Camera::default();
        c.frame([-10.0, -10.0, -10.0], [10.0, 10.0, 10.0]);
        let (eye, start) = (c.eye(), c.target);
        let mut right = c;
        right.look(50.0, 0.0);
        let moved = sub(right.eye(), eye);
        assert!(dot(moved, moved).sqrt() < 1e-3, "the camera stays put");
        // What was straight ahead is now on the left, at about the same height.
        let seen = project(&right.matrix(1.0), start);
        assert!(seen[0] < -0.1 && seen[1].abs() < 0.05, "{seen:?}");
        // Moving the mouse down looks down: what was ahead is now above the middle.
        let mut down = c;
        down.look(0.0, 50.0);
        assert!(project(&down.matrix(1.0), start)[1] > 0.01);
    }

    #[test]
    fn framing_centres_the_box() {
        let mut c = Camera::default();
        c.frame([10.0, 0.0, 20.0], [30.0, 4.0, 60.0]);
        let centre = project(&c.matrix(1.5), [20.0, 2.0, 40.0]);
        assert!(centre[0].abs() < 1e-4 && centre[1].abs() < 1e-4);
    }

    #[test]
    fn the_heading_is_the_nearest_grid_axis_and_right_is_a_quarter_turn_from_it() {
        let mut camera = Camera::default();
        for yaw in [0.0f32, 0.4, 1.0, 1.5, 2.3, 3.0, 4.0, 4.7, 5.5] {
            camera.yaw = yaw;
            let (forward, right) = camera.heading();
            assert_eq!(forward.iter().map(|v| v.abs()).sum::<i32>(), 1, "yaw {yaw}");
            assert_eq!(forward[1], 0);
            // The camera looks from (sin yaw, cos yaw) toward the middle: forward points against that.
            let look = [-yaw.sin(), -yaw.cos()];
            assert!(look[0] * forward[0] as f32 + look[1] * forward[2] as f32 > 0.5, "yaw {yaw}");
            // Right is forward turned a quarter turn clockwise seen from above.
            assert_eq!(right, [-forward[2], 0, forward[0]]);
        }
    }
}
