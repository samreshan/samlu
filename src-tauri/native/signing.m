#import <Foundation/Foundation.h>
#import <Security/Security.h>

/// Whether the running app carries a Developer ID signature (one with a team
/// identifier) rather than an ad-hoc one. macOS ties privacy permissions to
/// the signature, so ad-hoc builds lose Microphone and Accessibility access
/// whenever the app is replaced, and Settings warns about that before an
/// update.
bool samlu_is_developer_id_signed(void) {
  NSURL *bundle = [[NSBundle mainBundle] bundleURL];
  if (bundle == nil) {
    return false;
  }
  SecStaticCodeRef code = NULL;
  if (SecStaticCodeCreateWithPath((__bridge CFURLRef)bundle, kSecCSDefaultFlags, &code) != errSecSuccess
      || code == NULL) {
    return false;
  }
  CFDictionaryRef information = NULL;
  OSStatus status = SecCodeCopySigningInformation(code, kSecCSSigningInformation, &information);
  CFRelease(code);
  if (status != errSecSuccess || information == NULL) {
    return false;
  }
  NSDictionary *info = CFBridgingRelease(information);
  NSString *team = info[(__bridge NSString *)kSecCodeInfoTeamIdentifier];
  return [team isKindOfClass:[NSString class]] && team.length > 0;
}
