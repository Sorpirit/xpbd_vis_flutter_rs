import 'dart:typed_data';
import 'dart:ui';

import 'package:flutter/material.dart';
import 'package:xpbd_viz/camera.dart';
import 'package:xpbd_viz/src/rust/api/sim.dart';

/// Debug overlays that show what the solver is doing.
enum Gizmo { particles, constraints, velocities, contacts, grid }

/// Draws the scene from the flat position buffer Rust returns.
///
/// With [themed] on, bodies get their castle skin. The [gizmos] draw the raw
/// simulation data on top.
class ScenePainter extends CustomPainter {
  ScenePainter({
    required this.frame,
    required this.scene,
    required this.camera,
    required this.themed,
    required this.gizmos,
  }) : super(repaint: frame);

  final ValueNotifier<SimFrame?> frame;
  final SceneInfo scene;
  final Camera camera;
  final bool themed;
  final Set<Gizmo> gizmos;

  static const _wood = Color(0xFF7A5536);
  static const _kindColors = {
    BodyKind.rope: Colors.tealAccent,
    BodyKind.cloth: Colors.lightBlueAccent,
    BodyKind.soft: Colors.pinkAccent,
    BodyKind.stone: Colors.amber,
    BodyKind.metal: Colors.deepOrangeAccent,
    BodyKind.wood: Colors.limeAccent,
  };
  static const _flagColors = [
    [Color(0xFFC62828), Color(0xFFFFC107)],
    [Color(0xFF1565C0), Color(0xFFECEFF1)],
  ];

  // Scratch buffers, reused every frame.
  late final _slack = Float32List(scene.rests.length * 4);
  late final _stretched = Float32List(scene.rests.length * 4);
  late final _squashed = Float32List(scene.rests.length * 4);

  // Per flag: triangle indices and vertex colours, built on first use.
  final _flagIndices = <int, Uint16List>{};
  final _flagVertexColors = <int, Int32List>{};

  @override
  void paint(Canvas canvas, Size size) {
    canvas.save();
    camera.apply(canvas, size);
    _drawGround(canvas);

    final f = frame.value;
    if (f != null && f.positions.isNotEmpty) {
      final pos = f.positions;
      if (themed) {
        _drawProps(canvas, pos);
        var flag = 0;
        for (final body in scene.bodies) {
          switch (body.kind) {
            case BodyKind.stone || BodyKind.metal || BodyKind.wood:
              _drawBlock(canvas, pos, body);
            case BodyKind.cloth:
              _drawFlag(canvas, pos, body, flag++);
            case BodyKind.rope:
              _drawRope(canvas, pos, body);
            case BodyKind.soft:
              _drawSlime(canvas, pos, body);
          }
        }
      }
      if (gizmos.contains(Gizmo.grid)) _drawGrid(canvas, pos);
      if (gizmos.contains(Gizmo.constraints)) _drawConstraints(canvas, pos);
      if (gizmos.contains(Gizmo.particles)) _drawParticles(canvas, pos);
      if (gizmos.contains(Gizmo.velocities)) {
        _drawVelocities(canvas, pos, f.velocities);
      }
      if (gizmos.contains(Gizmo.contacts)) {
        _drawContacts(canvas, pos, f.contacts);
      }
    }
    canvas.restore();
  }

  // -- themed drawing -------------------------------------------------------

  void _drawGround(Canvas canvas) {
    final wall = scene.wallX, floor = scene.floorY;
    final earth = Paint()..color = const Color(0xFF1B2420);
    canvas.drawRect(
      Rect.fromLTRB(-wall - 40, floor, wall + 40, floor + 40),
      earth,
    );
    // cliffs where the room's side walls are
    canvas.drawRect(Rect.fromLTRB(-wall - 40, -40, -wall, floor), earth);
    canvas.drawRect(Rect.fromLTRB(wall, -40, wall + 40, floor), earth);
    canvas.drawLine(
      Offset(-wall, floor),
      Offset(wall, floor),
      Paint()
        ..color = const Color(0xFF5C9E48)
        ..strokeWidth = 0.06
        ..strokeCap = StrokeCap.round,
    );
  }

  /// Decoration only: the wooden frame the ram hangs from.
  void _drawProps(Canvas canvas, Float32List pos) {
    final wood = Paint()
      ..color = _wood
      ..strokeWidth = 0.09
      ..strokeCap = StrokeCap.round
      ..style = PaintingStyle.stroke;
    final floor = scene.floorY;

    // a gallows holding the rope's pinned end
    for (final body in scene.bodies) {
      if (body.kind != BodyKind.rope) continue;
      final px = pos[body.start * 2], py = pos[body.start * 2 + 1];
      canvas.drawPath(
        Path()
          ..moveTo(px - 2.2, floor)
          ..lineTo(px - 2.2, py - 0.12)
          ..lineTo(px + 0.15, py - 0.12)
          ..moveTo(px - 2.2, py + 0.5)
          ..lineTo(px - 1.6, py - 0.12),
        wood,
      );
    }
  }

  /// A rigid block: its four corner particles, pushed out by one radius so
  /// the quad covers the discs.
  void _drawBlock(Canvas canvas, Float32List pos, BodyInfo body) {
    final o = body.outline;
    var cx = 0.0, cy = 0.0;
    for (final i in o) {
      cx += pos[i * 2] / o.length;
      cy += pos[i * 2 + 1] / o.length;
    }
    final path = Path();
    for (var k = 0; k < o.length; k++) {
      final dx = pos[o[k] * 2] - cx, dy = pos[o[k] * 2 + 1] - cy;
      final grow = scene.radius * 1.35 / Offset(dx, dy).distance;
      final x = cx + dx * (1 + grow), y = cy + dy * (1 + grow);
      if (k == 0) {
        path.moveTo(x, y);
      } else {
        path.lineTo(x, y);
      }
    }
    path.close();

    final Color fill;
    if (body.kind == BodyKind.metal) {
      fill = const Color(0xFF465063);
    } else if (body.kind == BodyKind.wood) {
      fill = _wood;
    } else {
      // each stone gets its own shade of grey, stable across frames
      final shade = 0.42 + ((body.start * 2654435761) % 1000) / 1000 * 0.16;
      fill = HSLColor.fromAHSL(1, 32, 0.10, shade).toColor();
    }
    canvas.drawPath(path, Paint()..color = fill);
    canvas.drawPath(
      path,
      Paint()
        ..color = const Color(0xCC14161C)
        ..style = PaintingStyle.stroke
        ..strokeWidth = 0.025
        ..strokeJoin = StrokeJoin.round,
    );
  }

  /// A flag: the particle grid becomes a triangle mesh with striped colours,
  /// sent to the GPU in one drawVertices call.
  void _drawFlag(Canvas canvas, Float32List pos, BodyInfo body, int flag) {
    final cols = body.cols, rows = body.rows;
    final indices = _flagIndices.putIfAbsent(body.start, () {
      final list = Uint16List((cols - 1) * (rows - 1) * 6);
      var n = 0;
      for (var r = 0; r < rows - 1; r++) {
        for (var c = 0; c < cols - 1; c++) {
          final a = r * cols + c, b = a + 1, d = a + cols, e = d + 1;
          list.setAll(n, [a, b, d, b, e, d]);
          n += 6;
        }
      }
      return list;
    });
    final colors = _flagVertexColors.putIfAbsent(body.start, () {
      final palette = _flagColors[flag % _flagColors.length];
      final list = Int32List(cols * rows);
      for (var r = 0; r < rows; r++) {
        for (var c = 0; c < cols; c++) {
          list[r * cols + c] = palette[(r ~/ 2) % 2].toARGB32();
        }
      }
      return list;
    });
    // a view into the position buffer: no copy
    final vertices = Float32List.sublistView(
      pos,
      body.start * 2,
      (body.start + body.count) * 2,
    );
    canvas.drawVertices(
      Vertices.raw(
        VertexMode.triangles,
        vertices,
        colors: colors,
        indices: indices,
      ),
      BlendMode.modulate, // white paint times vertex colour = vertex colour
      Paint()..color = Colors.white,
    );
  }

  void _drawRope(Canvas canvas, Float32List pos, BodyInfo body) {
    final paint = Paint()
      ..color = const Color(0xFFB08D57)
      ..strokeWidth = 0.06
      ..strokeCap = StrokeCap.round
      ..strokeJoin = StrokeJoin.round;
    canvas.drawRawPoints(
      PointMode.polygon,
      Float32List.sublistView(
        pos,
        body.start * 2,
        (body.start + body.count) * 2,
      ),
      paint,
    );
    for (var i = 0; i + 1 < scene.ties.length; i += 2) {
      final a = scene.ties[i] * 2, b = scene.ties[i + 1] * 2;
      canvas.drawLine(
        Offset(pos[a], pos[a + 1]),
        Offset(pos[b], pos[b + 1]),
        paint,
      );
    }
  }

  /// A slime: a smooth closed curve through the ring, plus eyes.
  void _drawSlime(Canvas canvas, Float32List pos, BodyInfo body) {
    final o = body.outline;
    Offset at(int k) {
      final i = o[k % o.length] * 2;
      return Offset(pos[i], pos[i + 1]);
    }

    final path = Path();
    var centre = Offset.zero;
    for (var k = 0; k < o.length; k++) {
      final p = at(k), mid = (p + at(k + 1)) / 2;
      if (k == 0) {
        final start = (at(o.length - 1) + p) / 2;
        path.moveTo(start.dx, start.dy);
      }
      path.quadraticBezierTo(p.dx, p.dy, mid.dx, mid.dy);
      centre += p / o.length.toDouble();
    }
    path.close();
    canvas.drawPath(path, Paint()..color = const Color(0xE67BD66B));
    canvas.drawPath(
      path,
      Paint()
        ..color = const Color(0xFF2E7D32)
        ..style = PaintingStyle.stroke
        ..strokeWidth = 0.04,
    );
    for (final side in [-1, 1]) {
      final eye = centre + Offset(side * 0.09, -0.04);
      canvas.drawCircle(eye, 0.055, Paint()..color = Colors.white);
      canvas.drawCircle(
        eye + const Offset(0.015, 0.01),
        0.025,
        Paint()..color = Colors.black,
      );
    }
  }

  // -- gizmos ---------------------------------------------------------------

  void _drawParticles(Canvas canvas, Float32List pos) {
    for (final body in scene.bodies) {
      canvas.drawRawPoints(
        PointMode.points,
        Float32List.sublistView(
          pos,
          body.start * 2,
          (body.start + body.count) * 2,
        ),
        Paint()
          ..color = _kindColors[body.kind]!.withValues(
            alpha: themed ? 0.6 : 0.9,
          )
          ..strokeWidth = scene.radius * 2
          ..strokeCap = StrokeCap.round,
      );
    }
  }

  /// Every distance constraint, coloured by how far it is from its rest length.
  void _drawConstraints(Canvas canvas, Float32List pos) {
    var slack = 0, stretched = 0, squashed = 0;
    final pairs = scene.constraints;
    for (var k = 0; k < scene.rests.length; k++) {
      final a = pairs[k * 2] * 2, b = pairs[k * 2 + 1] * 2;
      final dx = pos[a] - pos[b], dy = pos[a + 1] - pos[b + 1];
      final strain = Offset(dx, dy).distance / scene.rests[k] - 1;
      final Float32List out;
      final int at;
      if (strain > 0.01) {
        out = _stretched;
        at = stretched;
        stretched += 4;
      } else if (strain < -0.01) {
        out = _squashed;
        at = squashed;
        squashed += 4;
      } else {
        out = _slack;
        at = slack;
        slack += 4;
      }
      out[at] = pos[a];
      out[at + 1] = pos[a + 1];
      out[at + 2] = pos[b];
      out[at + 3] = pos[b + 1];
    }
    void draw(Float32List buffer, int length, Color color) {
      if (length == 0) return;
      canvas.drawRawPoints(
        PointMode.lines,
        Float32List.sublistView(buffer, 0, length),
        Paint()
          ..color = color
          ..strokeWidth = 0.02,
      );
    }

    draw(_slack, slack, Colors.white54);
    draw(_squashed, squashed, Colors.lightBlueAccent);
    draw(_stretched, stretched, Colors.redAccent);
  }

  void _drawVelocities(Canvas canvas, Float32List pos, Float32List vel) {
    // velocities arrive one frame after the gizmo is switched on
    if (vel.length != pos.length) return;
    final segments = Float32List(pos.length * 2);
    for (var i = 0; i < pos.length; i += 2) {
      segments[i * 2] = pos[i];
      segments[i * 2 + 1] = pos[i + 1];
      segments[i * 2 + 2] = pos[i] + vel[i] * 0.05;
      segments[i * 2 + 3] = pos[i + 1] + vel[i + 1] * 0.05;
    }
    canvas.drawRawPoints(
      PointMode.lines,
      segments,
      Paint()
        ..color = Colors.yellowAccent
        ..strokeWidth = 0.015,
    );
  }

  void _drawContacts(Canvas canvas, Float32List pos, Uint32List pairs) {
    final segments = Float32List(pairs.length * 2);
    for (var i = 0; i < pairs.length; i++) {
      segments[i * 2] = pos[pairs[i] * 2];
      segments[i * 2 + 1] = pos[pairs[i] * 2 + 1];
    }
    final paint = Paint()
      ..color = Colors.redAccent
      ..strokeWidth = 0.035
      ..strokeCap = StrokeCap.round;
    canvas.drawRawPoints(PointMode.lines, segments, paint);
    canvas.drawRawPoints(PointMode.points, segments, paint..strokeWidth = 0.06);
  }

  /// The collision grid: only cells that currently hold a particle.
  void _drawGrid(Canvas canvas, Float32List pos) {
    final cell = scene.radius * 2;
    final seen = <int>{};
    final paint = Paint()
      ..color = Colors.greenAccent.withValues(alpha: 0.45)
      ..style = PaintingStyle.stroke
      ..strokeWidth = 0.008;
    for (var i = 0; i < pos.length; i += 2) {
      final cx = (pos[i] / cell).floor(), cy = (pos[i + 1] / cell).floor();
      if (seen.add(cx * 100000 + cy)) {
        canvas.drawRect(Rect.fromLTWH(cx * cell, cy * cell, cell, cell), paint);
      }
    }
  }

  @override
  bool shouldRepaint(ScenePainter oldDelegate) =>
      oldDelegate.themed != themed ||
      oldDelegate.gizmos != gizmos ||
      oldDelegate.scene != scene;
}

/// Full-screen background drawn by the GLSL shader in shaders/sky.frag.
/// Falls back to a plain gradient until the shader has loaded.
class SkyPainter extends CustomPainter {
  SkyPainter({
    required this.shader,
    required this.time,
    required this.camera,
    required this.floorY,
  }) : super(repaint: time);

  final FragmentShader? shader;
  final ValueNotifier<double> time;
  final Camera camera;
  final double floorY;

  @override
  void paint(Canvas canvas, Size size) {
    final rect = Offset.zero & size;
    final horizon = camera.toScreen(Offset(0, floorY), size).dy / size.height;
    final shader = this.shader;
    if (shader == null) {
      canvas.drawRect(
        rect,
        Paint()
          ..shader = const LinearGradient(
            begin: Alignment.topCenter,
            end: Alignment.bottomCenter,
            colors: [Color(0xFF121738), Color(0xFF784A78), Color(0xFFFA9457)],
          ).createShader(rect),
      );
      return;
    }
    // Uniforms are set by position, in the order the shader declares them.
    shader
      ..setFloat(0, size.width)
      ..setFloat(1, size.height)
      ..setFloat(2, time.value)
      ..setFloat(3, camera.center.dx)
      ..setFloat(4, horizon);
    canvas.drawRect(rect, Paint()..shader = shader);
  }

  @override
  bool shouldRepaint(SkyPainter oldDelegate) => oldDelegate.shader != shader;
}

/// Tiny line chart of recent values.
class SparklinePainter extends CustomPainter {
  SparklinePainter(this.values);
  final List<double> values;

  @override
  void paint(Canvas canvas, Size size) {
    if (values.length < 2) return;
    var lo = values.first, hi = values.first;
    for (final v in values) {
      if (v < lo) lo = v;
      if (v > hi) hi = v;
    }
    final range = (hi - lo).abs() < 1e-6 ? 1.0 : hi - lo;
    final path = Path();
    for (var i = 0; i < values.length; i++) {
      final x = size.width * i / (values.length - 1);
      final y = size.height * (1 - (values[i] - lo) / range);
      if (i == 0) {
        path.moveTo(x, y);
      } else {
        path.lineTo(x, y);
      }
    }
    canvas.drawPath(
      path,
      Paint()
        ..color = Colors.amberAccent
        ..style = PaintingStyle.stroke
        ..strokeWidth = 1.5,
    );
  }

  @override
  bool shouldRepaint(SparklinePainter oldDelegate) => true;
}
