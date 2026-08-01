#import <AVFoundation/AVFoundation.h>
#import <AppKit/AppKit.h>
#import <ApplicationServices/ApplicationServices.h>
#include <string.h>
#include <unistd.h>

static void samlu_on_main_sync(dispatch_block_t block) {
  if ([NSThread isMainThread]) {
    block();
  } else {
    dispatch_sync(dispatch_get_main_queue(), block);
  }
}

/// Microphone authorization as a plain integer so Rust can read it without
/// depending on the AVFoundation enum layout.
/// 0 = not determined, 1 = restricted, 2 = denied, 3 = authorized.
int samlu_microphone_authorization(void) {
  switch ([AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio]) {
    case AVAuthorizationStatusNotDetermined:
      return 0;
    case AVAuthorizationStatusRestricted:
      return 1;
    case AVAuthorizationStatusDenied:
      return 2;
    case AVAuthorizationStatusAuthorized:
      return 3;
  }
  return 1;
}

/// Asks macOS to show its own microphone prompt. Only does anything the first
/// time; afterwards the answer lives in System Settings and the call is a
/// no-op, which is why onboarding polls the status separately.
void samlu_request_microphone_access(void) {
  [AVCaptureDevice requestAccessForMediaType:AVMediaTypeAudio
                           completionHandler:^(BOOL granted) {
                             (void)granted;
                           }];
}

bool samlu_request_accessibility_access(void) {
  NSDictionary *options = @{
    (__bridge NSString *)kAXTrustedCheckOptionPrompt: @YES
  };
  return AXIsProcessTrustedWithOptions((__bridge CFDictionaryRef)options);
}

/// Captures the actual foreground application without System Events or an
/// Accessibility dependency. Returns -1 when there is no usable target.
int32_t samlu_frontmost_application(char *bundleBuffer, size_t bundleCapacity) {
  __block NSRunningApplication *application = nil;
  samlu_on_main_sync(^{
    application = [NSWorkspace sharedWorkspace].frontmostApplication;
  });
  if (application == nil) {
    return -1;
  }
  NSString *bundleIdentifier = application.bundleIdentifier;
  if (bundleBuffer != NULL && bundleCapacity > 0) {
    const char *value = bundleIdentifier.UTF8String ?: "";
    strlcpy(bundleBuffer, value, bundleCapacity);
  }
  return application.processIdentifier;
}

/// Brings the app that owned the original text field back to its Space. PID is
/// preferred; bundle ID is a fallback for apps that relaunched meanwhile.
bool samlu_activate_application(int32_t pid, const char *bundleIdentifier) {
  __block BOOL activated = NO;
  samlu_on_main_sync(^{
    NSRunningApplication *application =
      [NSRunningApplication runningApplicationWithProcessIdentifier:pid];
    if (application == nil && bundleIdentifier != NULL) {
      NSString *bundle = [NSString stringWithUTF8String:bundleIdentifier];
      application =
        [[NSRunningApplication runningApplicationsWithBundleIdentifier:bundle] firstObject];
    }
    if (application != nil) {
      activated = [application activateWithOptions:0];
    }
  });
  return activated;
}

/// Posts Cmd+V from Samlu's own process. This uses the Accessibility grant
/// belonging to Samlu directly and avoids osascript/System Events automation.
bool samlu_post_paste_shortcut(void) {
  if (!AXIsProcessTrusted()) {
    return false;
  }
  CGEventRef keyDown = CGEventCreateKeyboardEvent(NULL, (CGKeyCode)9, true);
  CGEventRef keyUp = CGEventCreateKeyboardEvent(NULL, (CGKeyCode)9, false);
  if (keyDown == NULL || keyUp == NULL) {
    if (keyDown != NULL) CFRelease(keyDown);
    if (keyUp != NULL) CFRelease(keyUp);
    return false;
  }
  CGEventSetFlags(keyDown, kCGEventFlagMaskCommand);
  CGEventSetFlags(keyUp, kCGEventFlagMaskCommand);
  // Enter at the HID edge of the event stream so the shortcut follows the
  // same path as a physical key press. Posting at the already-annotated edge
  // is less reliable for full-screen and hardened applications.
  CGEventPost(kCGHIDEventTap, keyDown);
  usleep(12000);
  CGEventPost(kCGHIDEventTap, keyUp);
  CFRelease(keyDown);
  CFRelease(keyUp);
  return true;
}
