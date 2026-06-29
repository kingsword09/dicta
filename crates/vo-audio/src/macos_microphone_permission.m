#import <AVFoundation/AVFoundation.h>
#import <Foundation/Foundation.h>
#import <dispatch/dispatch.h>

int vo_audio_request_microphone_permission(void) {
    @autoreleasepool {
        AVAuthorizationStatus status = [AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio];
        switch (status) {
        case AVAuthorizationStatusAuthorized:
            return 0;
        case AVAuthorizationStatusDenied:
        case AVAuthorizationStatusRestricted:
            return 1;
        case AVAuthorizationStatusNotDetermined: {
            dispatch_semaphore_t semaphore = dispatch_semaphore_create(0);
            __block BOOL granted = NO;
            [AVCaptureDevice requestAccessForMediaType:AVMediaTypeAudio completionHandler:^(BOOL allowed) {
                granted = allowed;
                dispatch_semaphore_signal(semaphore);
            }];
            dispatch_semaphore_wait(semaphore, DISPATCH_TIME_FOREVER);
            return granted ? 0 : 1;
        }
        default:
            return 2;
        }
    }
}
