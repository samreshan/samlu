#import <AppKit/AppKit.h>
#import <ApplicationServices/ApplicationServices.h>

/// Carbon hot keys, which the global shortcut plugin is built on, always need
/// a key code, so chords made only of modifiers (⌃⌥, ⌥⌘) are watched here
/// instead. The tap only listens; it never changes or swallows an event.

/// Bits Rust compares against: 1 = Control, 2 = Option, 4 = Shift, 8 = Command.
typedef void (*samlu_modifier_callback)(int32_t kind, uint32_t modifiers);

static samlu_modifier_callback samlu_modifier_handler = NULL;
static CFMachPortRef samlu_modifier_tap = NULL;

static uint32_t samlu_modifier_bits(CGEventFlags flags) {
  uint32_t bits = 0;
  if (flags & kCGEventFlagMaskControl) bits |= 1;
  if (flags & kCGEventFlagMaskAlternate) bits |= 2;
  if (flags & kCGEventFlagMaskShift) bits |= 4;
  if (flags & kCGEventFlagMaskCommand) bits |= 8;
  return bits;
}

static CGEventRef samlu_modifier_tap_callback(
  CGEventTapProxy proxy,
  CGEventType type,
  CGEventRef event,
  void *context
) {
  (void)proxy;
  (void)context;
  // macOS turns a tap off if a callback is ever slow; turn it straight back on.
  if (type == kCGEventTapDisabledByTimeout || type == kCGEventTapDisabledByUserInput) {
    if (samlu_modifier_tap != NULL) CGEventTapEnable(samlu_modifier_tap, true);
    return event;
  }
  samlu_modifier_callback handler = samlu_modifier_handler;
  if (handler == NULL) return event;
  uint32_t modifiers = samlu_modifier_bits(CGEventGetFlags(event));
  // 0 = the held modifiers changed, 1 = a key or click landed while they were held.
  handler(type == kCGEventFlagsChanged ? 0 : 1, modifiers);
  return event;
}

/// Starts listening on the main run loop. Idempotent. Returns false when macOS
/// refuses the tap, which happens until Samlu is allowed under Accessibility.
bool samlu_start_modifier_monitor(samlu_modifier_callback handler) {
  samlu_modifier_handler = handler;
  __block bool started = false;
  void (^start)(void) = ^{
    if (samlu_modifier_tap != NULL) {
      CGEventTapEnable(samlu_modifier_tap, true);
      started = true;
      return;
    }
    CGEventMask mask = CGEventMaskBit(kCGEventFlagsChanged) | CGEventMaskBit(kCGEventKeyDown) |
                       CGEventMaskBit(kCGEventLeftMouseDown) |
                       CGEventMaskBit(kCGEventRightMouseDown) |
                       CGEventMaskBit(kCGEventOtherMouseDown);
    CFMachPortRef tap = CGEventTapCreate(
      kCGSessionEventTap,
      kCGHeadInsertEventTap,
      kCGEventTapOptionListenOnly,
      mask,
      samlu_modifier_tap_callback,
      NULL
    );
    if (tap == NULL) return;
    CFRunLoopSourceRef source = CFMachPortCreateRunLoopSource(kCFAllocatorDefault, tap, 0);
    CFRunLoopAddSource(CFRunLoopGetMain(), source, kCFRunLoopCommonModes);
    CFRelease(source);
    CGEventTapEnable(tap, true);
    samlu_modifier_tap = tap;
    started = true;
  };
  if ([NSThread isMainThread]) {
    start();
  } else {
    dispatch_sync(dispatch_get_main_queue(), start);
  }
  return started;
}
