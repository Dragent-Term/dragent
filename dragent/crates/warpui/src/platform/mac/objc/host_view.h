#import <AppKit/AppKit.h>
#import <QuartzCore/QuartzCore.h>

@interface NSPasteboard (HostApp)
- (NSArray *)getFilePaths;
@end

/// HostContentView is the Content view of a host window.
// It is backed by a Metal CALayer.
@interface HostContentView : NSView <CALayerDelegate, NSTextInputClient>
- (HostContentView *)initWithFrame:(NSRect)frame
                    metalDevice:(id)metalDevice
             enableTitlebarDrag:(BOOL)enableTitlebarDrag
                       testMode:(BOOL)testMode;
- (void)setAsyncCallback:(BOOL)shouldAsync;
- (void)setPresentsWithTransaction:(BOOL)presentsWithTransaction;
- (BOOL)keyDownImpl:(NSEvent *)event;
@end
