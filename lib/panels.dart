import 'package:flutter/material.dart';

/// Translucent card used for every overlay. When [visible] flips it slides
/// off towards [hideTowards] and fades, using Flutter's implicit animations:
/// you only change the target value and the widget animates to it.
class Panel extends StatelessWidget {
  const Panel({
    super.key,
    required this.child,
    this.visible = true,
    this.hideTowards = Offset.zero,
  });

  final Widget child;
  final bool visible;
  final Offset hideTowards; // in units of the panel's own size

  @override
  Widget build(BuildContext context) {
    return IgnorePointer(
      ignoring: !visible,
      child: AnimatedSlide(
        offset: visible ? Offset.zero : hideTowards,
        duration: const Duration(milliseconds: 350),
        curve: Curves.easeOutCubic,
        child: AnimatedOpacity(
          opacity: visible ? 1 : 0,
          duration: const Duration(milliseconds: 250),
          child: Container(
            padding: const EdgeInsets.all(12),
            decoration: BoxDecoration(
              color: Colors.black.withValues(alpha: 0.6),
              borderRadius: BorderRadius.circular(12),
              border: Border.all(color: Colors.white24),
            ),
            child: child,
          ),
        ),
      ),
    );
  }
}

/// Monospaced text with fixed-width digits, so changing numbers do not jitter.
const monoStyle = TextStyle(
  fontFamily: 'monospace',
  fontSize: 12,
  fontFeatures: [FontFeature.tabularFigures()],
);
