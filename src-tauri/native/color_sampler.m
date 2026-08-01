#import <AppKit/AppKit.h>

typedef void (*SamluColorCallback)(const char *);
static NSColorSampler *samluSampler;

static NSWindowCollectionBehavior samlu_fullscreen_overlay_behavior(void) {
  if (@available(macOS 26.0, *)) {
    return NSWindowCollectionBehaviorCanJoinAllApplications;
  }
  return NSWindowCollectionBehaviorFullScreenAuxiliary;
}

static NSWindowCollectionBehavior samlu_passive_overlay_behavior(void) {
  NSWindowCollectionBehavior behavior =
    NSWindowCollectionBehaviorCanJoinAllSpaces |
    NSWindowCollectionBehaviorTransient |
    NSWindowCollectionBehaviorIgnoresCycle;
  return behavior | samlu_fullscreen_overlay_behavior();
}

static NSWindowCollectionBehavior samlu_focusable_overlay_behavior(void) {
  NSWindowCollectionBehavior behavior =
    NSWindowCollectionBehaviorMoveToActiveSpace |
    NSWindowCollectionBehaviorTransient |
    NSWindowCollectionBehaviorIgnoresCycle;
  return behavior | samlu_fullscreen_overlay_behavior();
}

static void samlu_apply_overlay_behavior(NSWindow *window) {
  // Status-level windows remain visible above normal and fullscreen app
  // content without activating Samlu or pulling focus from the editor.
  window.level = NSStatusWindowLevel;
  window.collectionBehavior = samlu_passive_overlay_behavior();
  window.hidesOnDeactivate = NO;
  window.animationBehavior = NSWindowAnimationBehaviorNone;
}

static void samlu_apply_focusable_overlay_behavior(NSWindow *window) {
  window.level = NSStatusWindowLevel;
  window.collectionBehavior = samlu_focusable_overlay_behavior();
  window.hidesOnDeactivate = NO;
  window.animationBehavior = NSWindowAnimationBehaviorNone;
}

static NSScreen *samlu_screen_under_pointer(void) {
  NSPoint pointer = [NSEvent mouseLocation];
  for (NSScreen *screen in [NSScreen screens]) {
    if (NSMouseInRect(pointer, screen.frame, NO)) {
      return screen;
    }
  }
  return [NSScreen mainScreen] ?: [[NSScreen screens] firstObject];
}

void samlu_pick_color(SamluColorCallback callback) {
  dispatch_async(dispatch_get_main_queue(), ^{
    samluSampler = [[NSColorSampler alloc] init];
    [samluSampler showSamplerWithSelectionHandler:^(NSColor *selectedColor) {
      if (selectedColor == nil) {
        callback(NULL);
        samluSampler = nil;
        return;
      }

      NSColor *rgb = [selectedColor colorUsingColorSpace:[NSColorSpace sRGBColorSpace]];
      if (rgb == nil) {
        callback(NULL);
        samluSampler = nil;
        return;
      }

      char hex[8];
      snprintf(
        hex,
        sizeof(hex),
        "#%02X%02X%02X",
        (int)lround(rgb.redComponent * 255.0),
        (int)lround(rgb.greenComponent * 255.0),
        (int)lround(rgb.blueComponent * 255.0)
      );
      callback(hex);
      samluSampler = nil;
    }];
  });
}

/// Whether the display under the pointer actually has a camera housing. The
/// Island hangs its bridge graphic off a real notch; on an external monitor
/// that graphic would just be an unexplained black tab, so it is not drawn.
bool samlu_active_display_has_notch(void) {
  __block BOOL notched = NO;
  void (^probe)(void) = ^{
    NSScreen *screen = samlu_screen_under_pointer();
    if (screen == nil) {
      return;
    }
    if (@available(macOS 12.0, *)) {
      notched = screen.safeAreaInsets.top > 0.0;
    }
  };
  if ([NSThread isMainThread]) {
    probe();
  } else {
    dispatch_sync(dispatch_get_main_queue(), probe);
  }
  return notched == YES;
}

void samlu_configure_overlay_window(void *windowPointer) {
  if (windowPointer == NULL) {
    return;
  }
  dispatch_async(dispatch_get_main_queue(), ^{
    NSWindow *window = (__bridge NSWindow *)windowPointer;
    samlu_apply_overlay_behavior(window);
  });
}

/// Reasserts fullscreen/Spaces behavior on every presentation and anchors the
/// overlay to whichever display currently contains the pointer. AppKit owns
/// this operation because its screen coordinates stay authoritative while a
/// fullscreen Space is active.
void samlu_present_overlay_at_cursor(
  void *windowPointer,
  double width,
  double height,
  double bottomMargin
) {
  if (windowPointer == NULL) {
    return;
  }
  dispatch_async(dispatch_get_main_queue(), ^{
    NSWindow *window = (__bridge NSWindow *)windowPointer;
    [window orderOut:nil];
    samlu_apply_overlay_behavior(window);

    NSScreen *targetScreen = samlu_screen_under_pointer();

    [window setContentSize:NSMakeSize(width, height)];
    if (targetScreen != nil) {
      NSRect frame = targetScreen.frame;
      NSPoint origin = NSMakePoint(
        NSMidX(frame) - width / 2.0,
        NSMinY(frame) + bottomMargin
      );
      [window setFrameOrigin:origin];
    }
    [window orderFrontRegardless];
  });
}

/// Moves a keyboard-driven overlay to the Space/display under the pointer
/// before making it key. Doing this as one AppKit operation prevents focusing
/// Samlu from switching back to the Space where Settings was last open.
void samlu_present_focusable_overlay_at_cursor(
  void *windowPointer,
  double width,
  double height,
  double topFraction
) {
  if (windowPointer == NULL) {
    return;
  }
  dispatch_async(dispatch_get_main_queue(), ^{
    NSWindow *window = (__bridge NSWindow *)windowPointer;
    [window orderOut:nil];
    samlu_apply_focusable_overlay_behavior(window);
    NSScreen *targetScreen = samlu_screen_under_pointer();

    [window setContentSize:NSMakeSize(width, height)];
    if (targetScreen != nil) {
      NSRect frame = targetScreen.frame;
      NSPoint origin = NSMakePoint(
        NSMidX(frame) - width / 2.0,
        NSMaxY(frame) - height - NSHeight(frame) * topFraction
      );
      [window setFrameOrigin:origin];
    }
    [window makeKeyAndOrderFront:nil];
    [NSApp activateIgnoringOtherApps:YES];
  });
}

/// Grows or shrinks an overlay downward, holding its top edge still. A
/// launcher list that changes length should extend below the field the user is
/// typing in, not shove that field up and down the screen.
void samlu_resize_overlay_keeping_top(void *windowPointer, double height) {
  if (windowPointer == NULL) {
    return;
  }
  dispatch_async(dispatch_get_main_queue(), ^{
    NSWindow *window = (__bridge NSWindow *)windowPointer;
    NSRect frame = window.frame;
    NSRect content = [window contentRectForFrameRect:frame];
    CGFloat chrome = NSHeight(frame) - NSHeight(content);
    CGFloat target = height + chrome;
    if (fabs(target - NSHeight(frame)) < 0.5) {
      return;
    }
    CGFloat top = NSMaxY(frame);
    frame.origin.y = top - target;
    frame.size.height = target;
    [window setFrame:frame display:YES animate:NO];
  });
}

void samlu_focus_overlay(void *windowPointer) {
  if (windowPointer == NULL) {
    return;
  }
  dispatch_async(dispatch_get_main_queue(), ^{
    NSWindow *window = (__bridge NSWindow *)windowPointer;
    [window orderOut:nil];
    samlu_apply_focusable_overlay_behavior(window);
    [window makeKeyAndOrderFront:nil];
    [NSApp activateIgnoringOtherApps:YES];
  });
}

/// Lets clicks fall through to whatever is behind the overlay. The pet turns
/// this off only while its card is on screen, so the bee itself never steals a
/// click during its flight.
void samlu_set_ignores_mouse_events(void *windowPointer, bool ignores) {
  if (windowPointer == NULL) {
    return;
  }
  dispatch_async(dispatch_get_main_queue(), ^{
    NSWindow *window = (__bridge NSWindow *)windowPointer;
    window.ignoresMouseEvents = ignores;
  });
}

void samlu_order_front_without_activating(void *windowPointer) {
  if (windowPointer == NULL) {
    return;
  }
  dispatch_async(dispatch_get_main_queue(), ^{
    NSWindow *window = (__bridge NSWindow *)windowPointer;
    [window orderOut:nil];
    samlu_apply_overlay_behavior(window);
    [window orderFrontRegardless];
  });
}

void samlu_set_background_mode(bool backgroundMode) {
  dispatch_async(dispatch_get_main_queue(), ^{
    [NSApp setActivationPolicy:
      backgroundMode
        ? NSApplicationActivationPolicyAccessory
        : NSApplicationActivationPolicyRegular
    ];
    if (!backgroundMode) {
      [NSApp activateIgnoringOtherApps:YES];
    }
  });
}
