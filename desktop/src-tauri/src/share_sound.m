// Your Mac's sound, for a screen share: everything that's playing except
// WatchSync itself. Friends' voices are played by WatchSync's WebKit helper
// and the movie by mpv, which WatchSync started; sending either back would
// make friends hear themselves, or the movie twice. A Core Audio process tap
// (macOS 14.2+) captures the rest; the processes WatchSync is responsible for
// are kept out, and the list is updated as they come and go.
#import <CoreAudio/AudioHardwareTapping.h>
#import <CoreAudio/CATapDescription.h>
#import <CoreAudio/CoreAudio.h>
#import <Foundation/Foundation.h>
#include <dlfcn.h>
#include <unistd.h>

/// Interleaved float frames; `channels` is 1 or 2.
typedef void (*ws_sound_cb)(void *ctx, const float *samples, uint32_t frames, uint32_t channels);

static AudioObjectID gTap = kAudioObjectUnknown;
static AudioObjectID gDevice = kAudioObjectUnknown;
static AudioDeviceIOProcID gProc = NULL;
static dispatch_queue_t gQueue = NULL;
static AudioObjectPropertyListenerBlock gListener = NULL;
static NSArray<NSNumber *> *gExcluded = nil;
static id gDesc = nil; // CATapDescription

static const AudioObjectPropertyAddress kProcessList = {
    kAudioHardwarePropertyProcessObjectList, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain};

int ws_sound_available(void) {
  if (@available(macOS 14.2, *)) return 1;
  return 0;
}

/// The process that macOS holds responsible for `pid` (an app, for its
/// helpers and the programs it starts).
static pid_t responsible(pid_t pid) {
  static pid_t (*fn)(pid_t) = NULL;
  static dispatch_once_t once;
  dispatch_once(&once, ^{
    fn = (pid_t (*)(pid_t))dlsym(RTLD_DEFAULT, "responsibility_get_pid_responsible_for_pid");
  });
  return fn ? fn(pid) : pid;
}

/// Audio process objects that are WatchSync, or that WatchSync started.
static NSArray<NSNumber *> *ours(void) {
  UInt32 size = 0;
  if (AudioObjectGetPropertyDataSize(kAudioObjectSystemObject, &kProcessList, 0, NULL, &size) != noErr) return @[];
  UInt32 n = size / sizeof(AudioObjectID);
  AudioObjectID *ids = calloc(n ? n : 1, sizeof(AudioObjectID));
  NSMutableArray *out = [NSMutableArray array];
  if (AudioObjectGetPropertyData(kAudioObjectSystemObject, &kProcessList, 0, NULL, &size, ids) == noErr) {
    pid_t me = getpid();
    // Run from Terminal (development), the app isn't its own responsible
    // process; whatever it starts shares that one.
    pid_t mine = responsible(me);
    for (UInt32 i = 0; i < size / sizeof(AudioObjectID); i++) {
      AudioObjectPropertyAddress a = {kAudioProcessPropertyPID, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain};
      pid_t pid = 0;
      UInt32 s = sizeof(pid);
      if (AudioObjectGetPropertyData(ids[i], &a, 0, NULL, &s, &pid) != noErr) continue;
      pid_t r = responsible(pid);
      if (pid == me || r == me || r == mine) [out addObject:@(ids[i])];
    }
  }
  free(ids);
  return [out sortedArrayUsingSelector:@selector(compare:)];
}

/// Keep up with WatchSync's processes (the movie opening, WebKit starting
/// its audio).
static void refreshExcluded(void) API_AVAILABLE(macos(14.2)) {
  if (gTap == kAudioObjectUnknown || !gDesc) return;
  NSArray *now = ours();
  if ([now isEqualToArray:gExcluded]) return;
  gExcluded = now;
  CATapDescription *desc = gDesc;
  desc.processes = now;
  AudioObjectPropertyAddress a = {kAudioTapPropertyDescription, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain};
  CFTypeRef ref = (__bridge CFTypeRef)desc;
  AudioObjectSetPropertyData(gTap, &a, 0, NULL, sizeof(ref), &ref);
}

void ws_sound_stop(void) {
  if (@available(macOS 14.2, *)) {
    if (gListener) {
      AudioObjectRemovePropertyListenerBlock(kAudioObjectSystemObject, &kProcessList, gQueue, gListener);
      gListener = NULL;
    }
    if (gDevice != kAudioObjectUnknown && gProc) {
      AudioDeviceStop(gDevice, gProc);
      AudioDeviceDestroyIOProcID(gDevice, gProc);
    }
    gProc = NULL;
    // Nothing more arrives after this: the caller may free its context.
    if (gQueue) dispatch_sync(gQueue, ^{});
    if (gDevice != kAudioObjectUnknown) AudioHardwareDestroyAggregateDevice(gDevice);
    gDevice = kAudioObjectUnknown;
    if (gTap != kAudioObjectUnknown) AudioHardwareDestroyProcessTap(gTap);
    gTap = kAudioObjectUnknown;
    gDesc = nil;
    gExcluded = nil;
  }
}

/// Start capturing; `cb` is called on a private queue. Returns 0 and the
/// sample rate, or a Core Audio error (-1: needs macOS 14.2).
int ws_sound_start(void *ctx, ws_sound_cb cb, double *rate) {
  if (@available(macOS 14.2, *)) {
    ws_sound_stop();
    if (!gQueue) gQueue = dispatch_queue_create("watchsync.share-sound", DISPATCH_QUEUE_SERIAL);

    gExcluded = ours();
    CATapDescription *desc = [[CATapDescription alloc] initStereoGlobalTapButExcludeProcesses:gExcluded];
    desc.name = @"WatchSync screen share";
    desc.UUID = [NSUUID UUID];
    desc.privateTap = YES;
    desc.muteBehavior = CATapUnmuted; // you keep hearing it too
    gDesc = desc;
    OSStatus st = AudioHardwareCreateProcessTap(desc, &gTap);
    if (st != noErr) {
      gTap = kAudioObjectUnknown;
      ws_sound_stop();
      return st;
    }

    AudioObjectPropertyAddress fa = {kAudioTapPropertyFormat, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain};
    AudioStreamBasicDescription format = {0};
    UInt32 fs = sizeof(format);
    st = AudioObjectGetPropertyData(gTap, &fa, 0, NULL, &fs, &format);
    if (st != noErr || format.mFormatID != kAudioFormatLinearPCM || !(format.mFormatFlags & kAudioFormatFlagIsFloat) ||
        format.mBitsPerChannel != 32) {
      ws_sound_stop();
      return st != noErr ? st : 'fmt?';
    }
    *rate = format.mSampleRate;

    NSDictionary *agg = @{
      @kAudioAggregateDeviceNameKey : @"WatchSync screen share",
      @kAudioAggregateDeviceUIDKey : [NSUUID UUID].UUIDString,
      @kAudioAggregateDeviceIsPrivateKey : @YES,
      @kAudioAggregateDeviceIsStackedKey : @NO,
      @kAudioAggregateDeviceTapAutoStartKey : @YES,
      @kAudioAggregateDeviceTapListKey : @[ @{
        @kAudioSubTapUIDKey : desc.UUID.UUIDString,
        @kAudioSubTapDriftCompensationKey : @YES,
      } ],
    };
    st = AudioHardwareCreateAggregateDevice((__bridge CFDictionaryRef)agg, &gDevice);
    if (st != noErr) {
      gDevice = kAudioObjectUnknown;
      ws_sound_stop();
      return st;
    }

    st = AudioDeviceCreateIOProcIDWithBlock(&gProc, gDevice, gQueue,
        ^(const AudioTimeStamp *now, const AudioBufferList *in, const AudioTimeStamp *inTime, AudioBufferList *out,
          const AudioTimeStamp *outTime) {
          if (!in || in->mNumberBuffers == 0) return;
          if (in->mNumberBuffers == 1) {
            // Interleaved.
            const AudioBuffer b = in->mBuffers[0];
            UInt32 ch = b.mNumberChannels ? b.mNumberChannels : 1;
            if (ch > 2 || !b.mData) return;
            cb(ctx, b.mData, b.mDataByteSize / (sizeof(float) * ch), ch);
          } else {
            // One buffer per channel: interleave the first two.
            const AudioBuffer l = in->mBuffers[0], r = in->mBuffers[1];
            if (!l.mData || !r.mData) return;
            UInt32 frames = MIN(l.mDataByteSize, r.mDataByteSize) / sizeof(float);
            float *both = malloc(sizeof(float) * frames * 2);
            if (!both) return;
            for (UInt32 i = 0; i < frames; i++) {
              both[2 * i] = ((const float *)l.mData)[i];
              both[2 * i + 1] = ((const float *)r.mData)[i];
            }
            cb(ctx, both, frames, 2);
            free(both);
          }
        });
    if (st == noErr) st = AudioDeviceStart(gDevice, gProc);
    if (st != noErr) {
      ws_sound_stop();
      return st;
    }

    gListener = ^(UInt32 n, const AudioObjectPropertyAddress *addrs) {
      refreshExcluded();
    };
    AudioObjectAddPropertyListenerBlock(kAudioObjectSystemObject, &kProcessList, gQueue, gListener);
    return 0;
  }
  return -1;
}
