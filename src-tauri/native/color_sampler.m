#import <AppKit/AppKit.h>
#import <CoreGraphics/CoreGraphics.h>

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

/// The display holding the window you are working in: the frontmost app's
/// frontmost on-screen window. The pointer is only a fallback, because after
/// Command-Tab, a Mission Control switch or a keyboard-driven focus change the
/// pointer is often still resting on another display.
///
/// Window bounds and owner come from the window server and need no Screen
/// Recording permission; only window titles would.
static NSScreen *samlu_active_screen(void) {
  NSRunningApplication *front = [[NSWorkspace sharedWorkspace] frontmostApplication];
  NSArray<NSScreen *> *screens = [NSScreen screens];
  if (front != nil && screens.count > 0) {
    pid_t pid = front.processIdentifier;
    CFArrayRef list = CGWindowListCopyWindowInfo(
      kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
      kCGNullWindowID
    );
    if (list != NULL) {
      NSArray *windows = CFBridgingRelease(list);
      // Window-server bounds are top-left based on the primary display;
      // AppKit screen frames are bottom-left based on the same display.
      CGFloat primaryTop = NSMaxY(screens.firstObject.frame);
      // The list is ordered front to back, so the first normal-layer window
      // the app owns is the one it is showing you.
      for (NSDictionary *info in windows) {
        if ([info[(__bridge id)kCGWindowOwnerPID] intValue] != pid) {
          continue;
        }
        if ([info[(__bridge id)kCGWindowLayer] intValue] != 0) {
          continue;
        }
        CGRect bounds;
        CFDictionaryRef boundsInfo = (__bridge CFDictionaryRef)info[(__bridge id)kCGWindowBounds];
        if (boundsInfo == NULL || !CGRectMakeWithDictionaryRepresentation(boundsInfo, &bounds)) {
          continue;
        }
        if (bounds.size.width < 80 || bounds.size.height < 60) {
          continue;
        }
        NSPoint center = NSMakePoint(CGRectGetMidX(bounds), primaryTop - CGRectGetMidY(bounds));
        for (NSScreen *screen in screens) {
          if (NSPointInRect(center, screen.frame)) {
            return screen;
          }
        }
      }
    }
  }
  return samlu_screen_under_pointer();
}

/// Writes the active display's frame, in AppKit points, as x, y, width,
/// height. Callers anchor a whole session to it.
bool samlu_active_screen_frame(double *out) {
  if (out == NULL) {
    return false;
  }
  __block NSRect frame = NSZeroRect;
  void (^probe)(void) = ^{
    NSScreen *screen = samlu_active_screen();
    if (screen != nil) {
      frame = screen.frame;
    }
  };
  if ([NSThread isMainThread]) {
    probe();
  } else {
    dispatch_sync(dispatch_get_main_queue(), probe);
  }
  if (NSIsEmptyRect(frame)) {
    return false;
  }
  out[0] = frame.origin.x;
  out[1] = frame.origin.y;
  out[2] = frame.size.width;
  out[3] = frame.size.height;
  return true;
}

/// Sets an overlay's frame in AppKit points. Tauri's physical positions are
/// converted with the scale of the display the window is on *now*, so moving
/// between a Retina and a non-Retina display through them lands the window in
/// the wrong place, usually back on the display it came from.
void samlu_set_overlay_frame(
  void *windowPointer,
  double x,
  double y,
  double width,
  double height,
  bool present
) {
  if (windowPointer == NULL) {
    return;
  }
  dispatch_async(dispatch_get_main_queue(), ^{
    NSWindow *window = (__bridge NSWindow *)windowPointer;
    if (present) {
      // Ordering out first lets a window that last showed on another Space
      // arrive on the one you are looking at now.
      [window orderOut:nil];
      samlu_apply_overlay_behavior(window);
    }
    NSRect content = NSMakeRect(x, y, width, height);
    [window setFrame:[window frameRectForContentRect:content] display:YES animate:NO];
    if (present) {
      [window orderFrontRegardless];
    }
  });
}

/// Hangs an overlay from the top centre of the active display, keeping its
/// current size. Used by the Samlu Island.
void samlu_center_overlay_at_top(void *windowPointer) {
  if (windowPointer == NULL) {
    return;
  }
  dispatch_async(dispatch_get_main_queue(), ^{
    NSWindow *window = (__bridge NSWindow *)windowPointer;
    NSScreen *screen = samlu_active_screen();
    if (screen == nil) {
      return;
    }
    NSRect frame = window.frame;
    NSRect bounds = screen.frame;
    frame.origin.x = NSMidX(bounds) - NSWidth(frame) / 2.0;
    frame.origin.y = NSMaxY(bounds) - NSHeight(frame);
    [window setFrame:frame display:YES animate:NO];
  });
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
    NSScreen *screen = samlu_active_screen();
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

/// Moves a keyboard-driven overlay to the active Space and display before
/// making it key. Doing this as one AppKit operation prevents focusing
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
    NSScreen *targetScreen = samlu_active_screen();

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

/// Lets clicks fall through to whatever is behind the overlay, so a surface
/// that only reports status never steals a click.
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

/// Brings the regular Settings window forward as one AppKit transaction. The
/// old sequence changed activation policy asynchronously and immediately asked
/// Tauri to show/focus, which could leave Samlu active but invisible.
bool samlu_present_application_window(void *windowPointer) {
  if (windowPointer == NULL) {
    return false;
  }
  dispatch_async(dispatch_get_main_queue(), ^{
    NSWindow *window = (__bridge NSWindow *)windowPointer;
    [NSApp setActivationPolicy:NSApplicationActivationPolicyRegular];
    [NSApp activateIgnoringOtherApps:YES];
    [window deminiaturize:nil];
    [window makeKeyAndOrderFront:nil];
  });
  return true;
}
