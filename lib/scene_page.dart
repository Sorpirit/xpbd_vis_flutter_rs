import 'dart:math' as math;
import 'dart:ui';

import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/scheduler.dart';
import 'package:flutter/services.dart';
import 'package:xpbd_viz/camera.dart';
import 'package:xpbd_viz/panels.dart';
import 'package:xpbd_viz/scene_painter.dart';
import 'package:xpbd_viz/src/rust/api/sim.dart';

class ScenePage extends StatefulWidget {
  const ScenePage({super.key});

  @override
  State<ScenePage> createState() => _ScenePageState();
}

class _ScenePageState extends State<ScenePage> with TickerProviderStateMixin {
  late final Sim _sim;
  late final SceneInfo _scene;
  late final Ticker _ticker;

  // Notifiers let each layer repaint on its own, without rebuilding the page.
  final _frame = ValueNotifier<SimFrame?>(null); // repaints the scene
  final _time = ValueNotifier<double>(0); // repaints the sky
  final _hud = ValueNotifier<int>(0); // refreshes the stats text
  Duration _lastHud = Duration.zero;

  // camera
  final _camera = Camera();
  late final AnimationController _cameraHome; // drives the "reset view" glide
  Offset _homeFromCenter = Camera.home;
  double _homeFromZoom = 1;
  bool _panning = false;

  // sky shader, loaded asynchronously
  FragmentShader? _skyShader;

  // live controls
  double _substeps = 10;
  double _gravity = 9.81;
  double _compliance = 0;
  double _damping = 0;
  bool _paused = false;
  bool _themed = true;
  bool _sky = true;
  Set<Gizmo> _gizmos = {};

  // which panels are open
  bool _showStats = true;
  bool _showControls = true;

  // input waiting to be sent with the next physics call
  Offset? _dragWorld; // cursor in metres while the mouse is held
  Offset? _fireAt; // double-click target
  bool _resetRequested = false;
  bool _stepRequested = false;
  Duration _lastDown = Duration.zero;
  Offset _lastDownAt = Offset.zero;

  // frame pacing
  Duration _lastTick = Duration.zero;
  double _pendingDt = 0; // real time not yet handed to the physics
  bool _busy = false; // a physics call is in flight on the worker thread

  // performance
  double _fps = 0;
  double _callMs = 0;
  int _steps = 0;
  final List<double> _energy = [];

  @override
  void initState() {
    super.initState();
    _sim = Sim.scene();
    _scene = _sim.info(); // static: which particles form which body
    _ticker = createTicker(_onTick)..start();
    _cameraHome =
        AnimationController(
          vsync: this,
          duration: const Duration(milliseconds: 450),
        )..addListener(() {
          final t = Curves.easeOutCubic.transform(_cameraHome.value);
          _camera.center = Offset.lerp(_homeFromCenter, Camera.home, t)!;
          _camera.zoom = lerpDouble(_homeFromZoom, 1, t)!;
        });
    _loadSky();
    // Right-drag pans the camera, so keep the browser's menu out of the way.
    if (kIsWeb) BrowserContextMenu.disableContextMenu();
  }

  Future<void> _loadSky() async {
    try {
      final program = await FragmentProgram.fromAsset('shaders/sky.frag');
      if (mounted) setState(() => _skyShader = program.fragmentShader());
    } catch (_) {
      // No shader support here: the sky stays a plain gradient.
    }
  }

  /// Runs once per display frame, on the UI thread. It never waits for the
  /// physics: it only measures real time and kicks off the next call.
  void _onTick(Duration elapsed) {
    final frameDt = (elapsed - _lastTick).inMicroseconds / 1e6;
    _lastTick = elapsed;
    if (frameDt > 0) _fps = _fps * 0.9 + (1 / frameDt) * 0.1;
    if (_sky) _time.value = elapsed.inMicroseconds / 1e6;
    _pendingDt += frameDt;
    if (_busy) return; // physics is behind: keep showing the last frame
    final dt = _pendingDt;
    _pendingDt = 0;
    _advance(dt);
  }

  Future<void> _advance(double dt) async {
    _busy = true;
    final drag = _dragWorld, fire = _fireAt;
    final input = SimInput(
      gravity: _gravity,
      compliance: _compliance,
      damping: _damping,
      substeps: _substeps.round(),
      paused: _paused,
      reset: _resetRequested,
      drag: drag == null ? null : Vec2(x: drag.dx, y: drag.dy),
      fire: fire == null ? null : Vec2(x: fire.dx, y: fire.dy),
      stepOnce: _stepRequested,
      debug: _gizmos.contains(Gizmo.velocities),
    );
    _resetRequested = false;
    _stepRequested = false;
    _fireAt = null;
    final stopwatch = Stopwatch()..start();
    try {
      // Runs on a Rust worker thread; the UI keeps rendering meanwhile.
      final frame = await _sim.advance(frameDt: dt, input: input);
      if (!mounted) return;
      _frame.value = frame; // repaints only the canvas
      _steps = frame.steps;
      _callMs = _callMs * 0.9 + stopwatch.elapsedMicroseconds / 1000 * 0.1;
      if (input.reset) _energy.clear();
      if (frame.steps > 0) {
        _energy.add(frame.stats.kinetic + frame.stats.potential);
        if (_energy.length > 180) _energy.removeAt(0);
      }
      // Text nobody can read at 165 Hz: refresh the stats 10 times a second.
      if (_lastTick - _lastHud > const Duration(milliseconds: 100)) {
        _lastHud = _lastTick;
        _hud.value++;
      }
    } finally {
      _busy = false;
    }
  }

  // -- pointer input ----------------------------------------------------------

  void _onPointerDown(PointerDownEvent e, Size size) {
    if (e.kind == PointerDeviceKind.mouse && e.buttons != kPrimaryMouseButton) {
      _panning = true; // right or middle button moves the camera
      return;
    }
    final world = _camera.toWorld(e.localPosition, size);
    final quick = e.timeStamp - _lastDown < const Duration(milliseconds: 300);
    if (quick && (e.localPosition - _lastDownAt).distance < 24) {
      _fireAt = world; // double-click: throw a slime here
      _lastDown = Duration.zero;
      return;
    }
    _lastDown = e.timeStamp;
    _lastDownAt = e.localPosition;
    _dragWorld = world;
  }

  void _onPointerMove(PointerMoveEvent e, Size size) {
    if (_panning) {
      _cameraHome.stop();
      _camera.pan(e.delta, size);
    } else if (_dragWorld != null) {
      _dragWorld = _camera.toWorld(e.localPosition, size);
    }
  }

  void _onPointerEnd() {
    _panning = false;
    _dragWorld = null;
  }

  void _onPointerSignal(PointerSignalEvent e, Size size) {
    if (e is PointerScrollEvent) {
      _cameraHome.stop();
      _camera.zoomAt(e.localPosition, size, math.exp(-e.scrollDelta.dy / 400));
    }
  }

  void _resetView() {
    _homeFromCenter = _camera.center;
    _homeFromZoom = _camera.zoom;
    _cameraHome.forward(from: 0);
  }

  @override
  void dispose() {
    _ticker.dispose();
    _cameraHome.dispose();
    _frame.dispose();
    _time.dispose();
    _hud.dispose();
    super.dispose();
  }

  // -- widgets ----------------------------------------------------------------

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      body: Stack(
        children: [
          Positioned.fill(
            child: RepaintBoundary(
              child: CustomPaint(
                painter: SkyPainter(
                  shader: _sky ? _skyShader : null,
                  time: _time,
                  camera: _camera,
                  floorY: _scene.floorY,
                ),
              ),
            ),
          ),
          Positioned.fill(child: _buildCanvas()),
          Positioned(
            top: 16,
            left: 16,
            bottom: 84,
            width: 300,
            child: SingleChildScrollView(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  SizedBox(
                    width: 230,
                    child: Panel(
                      visible: _showStats,
                      hideTowards: const Offset(-1.2, 0),
                      child: RepaintBoundary(
                        child: ValueListenableBuilder(
                          valueListenable: _hud,
                          builder: (context, _, _) => _buildStats(),
                        ),
                      ),
                    ),
                  ),
                ],
              ),
            ),
          ),
          Positioned(
            top: 16,
            right: 16,
            bottom: 84,
            width: 300,
            child: SingleChildScrollView(
              child: Panel(
                visible: _showControls,
                hideTowards: const Offset(1.2, 0),
                child: _buildControls(),
              ),
            ),
          ),
          Positioned(
            left: 0,
            right: 0,
            bottom: 16,
            child: Center(child: _buildToolbar()),
          ),
        ],
      ),
    );
  }

  Widget _buildCanvas() {
    return LayoutBuilder(
      builder: (context, constraints) {
        final size = constraints.biggest;
        // Listener gives raw pointer events, so we can tell mouse buttons
        // apart and read the scroll wheel.
        return Listener(
          onPointerDown: (e) => _onPointerDown(e, size),
          onPointerMove: (e) => _onPointerMove(e, size),
          onPointerUp: (_) => _onPointerEnd(),
          onPointerCancel: (_) => _onPointerEnd(),
          onPointerSignal: (e) => _onPointerSignal(e, size),
          child: MouseRegion(
            cursor: SystemMouseCursors.grab,
            child: RepaintBoundary(
              child: CustomPaint(
                size: size,
                painter: ScenePainter(
                  frame: _frame,
                  scene: _scene,
                  camera: _camera,
                  themed: _themed,
                  gizmos: _gizmos,
                ),
              ),
            ),
          ),
        );
      },
    );
  }

  Widget _buildStats() {
    final s = _frame.value?.stats;
    if (s == null) return const SizedBox.shrink();
    final total = s.kinetic + s.potential;
    return DefaultTextStyle.merge(
      style: monoStyle,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text('FPS          ${_fps.toStringAsFixed(0)}'),
          Text('Physics call ${_callMs.toStringAsFixed(2)} ms'),
          Text('Steps/frame  $_steps  (fixed 60 Hz)'),
          Text('Particles    ${s.particles}'),
          Text('Constraints  ${s.constraints}'),
          Text('Contacts     ${s.contacts}'),
          const Divider(),
          Text('Kinetic      ${s.kinetic.toStringAsFixed(0)} J'),
          Text('Potential    ${s.potential.toStringAsFixed(0)} J'),
          Text('Total        ${total.toStringAsFixed(0)} J'),
          Text('Max stretch  ${(s.maxStretch * 100).toStringAsFixed(1)} %'),
          const SizedBox(height: 8),
          SizedBox(
            height: 40,
            width: double.infinity,
            child: CustomPaint(painter: SparklinePainter(_energy)),
          ),
        ],
      ),
    );
  }

  Widget _buildControls() {
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        _slider('Substeps', _substeps, 1, 40, 0, (v) => _substeps = v),
        _slider('Gravity', _gravity, 0, 20, 2, (v) => _gravity = v),
        _slider('Compliance', _compliance, 0, 0.02, 4, (v) => _compliance = v),
        _slider('Damping', _damping, 0, 5, 2, (v) => _damping = v),
        const Divider(),
        Row(
          children: [
            const Expanded(child: Text('Castle skin')),
            Switch(
              value: _themed,
              onChanged: (v) => setState(() {
                _themed = v;
                // never leave the canvas empty
                if (!v && _gizmos.isEmpty) _gizmos = {Gizmo.particles};
              }),
            ),
          ],
        ),
        Row(
          children: [
            const Expanded(child: Text('Shader sky')),
            Switch(value: _sky, onChanged: (v) => setState(() => _sky = v)),
          ],
        ),
        const SizedBox(height: 4),
        Text('Debug gizmos', style: Theme.of(context).textTheme.labelSmall),
        const SizedBox(height: 6),
        Wrap(
          spacing: 6,
          runSpacing: 6,
          children: [
            for (final gizmo in Gizmo.values)
              FilterChip(
                label: Text(gizmo.name),
                selected: _gizmos.contains(gizmo),
                visualDensity: VisualDensity.compact,
                onSelected: (on) => setState(() {
                  // a new set, so the painter sees that it changed
                  _gizmos = on
                      ? {..._gizmos, gizmo}
                      : _gizmos.difference({gizmo});
                }),
              ),
          ],
        ),
      ],
    );
  }

  Widget _slider(
    String label,
    double value,
    double min,
    double max,
    int decimals,
    ValueChanged<double> assign,
  ) {
    return Row(
      children: [
        SizedBox(width: 82, child: Text(label)),
        Expanded(
          child: Slider(
            value: value,
            min: min,
            max: max,
            onChanged: (v) => setState(() => assign(v)),
          ),
        ),
        SizedBox(width: 48, child: Text(value.toStringAsFixed(decimals))),
      ],
    );
  }

  Widget _buildToolbar() {
    return Panel(
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          IconButton(
            tooltip: _paused ? 'Resume' : 'Pause',
            onPressed: () => setState(() => _paused = !_paused),
            // The key tells AnimatedSwitcher the icon changed, so it
            // spins and fades from one to the other.
            icon: AnimatedSwitcher(
              duration: const Duration(milliseconds: 200),
              transitionBuilder: (child, animation) => RotationTransition(
                turns: Tween(begin: 0.75, end: 1.0).animate(animation),
                child: FadeTransition(opacity: animation, child: child),
              ),
              child: Icon(
                _paused ? Icons.play_arrow : Icons.pause,
                key: ValueKey(_paused),
              ),
            ),
          ),
          IconButton(
            tooltip: 'Step once (while paused)',
            icon: const Icon(Icons.skip_next),
            onPressed: _paused ? () => _stepRequested = true : null,
          ),
          IconButton(
            tooltip: 'Rebuild the castle',
            icon: const Icon(Icons.refresh),
            onPressed: () => _resetRequested = true,
          ),
          IconButton(
            tooltip: 'Reset view',
            icon: const Icon(Icons.center_focus_strong),
            onPressed: _resetView,
          ),
          const SizedBox(height: 24, child: VerticalDivider()),
          IconButton(
            tooltip: 'Stats',
            isSelected: _showStats,
            icon: const Icon(Icons.speed),
            onPressed: () => setState(() => _showStats = !_showStats),
          ),
          IconButton(
            tooltip: 'Controls',
            isSelected: _showControls,
            icon: const Icon(Icons.tune),
            onPressed: () => setState(() => _showControls = !_showControls),
          ),
          const SizedBox(width: 8),
          Text(
            'drag: pull · double-click: fire slime · right-drag: pan · scroll: zoom',
            style: Theme.of(context).textTheme.labelSmall
                ?.copyWith(color: Colors.white60),
          ),
          const SizedBox(width: 8),
        ],
      ),
    );
  }
}
