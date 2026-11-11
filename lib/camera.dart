import 'dart:math' as math;
import 'dart:ui';

/// Maps between world metres (what Rust simulates) and screen pixels.
class Camera {
  static const home = Offset(0, 2.3);

  Offset center = home; // world point shown at the middle of the screen
  double zoom = 1;

  /// Pixels per metre. At zoom 1 the whole 12 m wide room fits on screen.
  double scale(Size size) =>
      math.min(size.width / 12.6, size.height / 6.8) * zoom;

  Offset toWorld(Offset screen, Size size) =>
      (screen - size.center(Offset.zero)) / scale(size) + center;

  Offset toScreen(Offset world, Size size) =>
      (world - center) * scale(size) + size.center(Offset.zero);

  /// After this, drawing calls take world coordinates.
  void apply(Canvas canvas, Size size) {
    canvas.translate(size.width / 2, size.height / 2);
    canvas.scale(scale(size));
    canvas.translate(-center.dx, -center.dy);
  }

  /// Zoom while keeping the world point under the cursor where it is.
  void zoomAt(Offset screen, Size size, double factor) {
    final before = toWorld(screen, size);
    zoom = (zoom * factor).clamp(0.5, 8.0);
    center += before - toWorld(screen, size);
  }

  void pan(Offset screenDelta, Size size) {
    center -= screenDelta / scale(size);
  }
}
