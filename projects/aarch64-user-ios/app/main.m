#import <UIKit/UIKit.h>

#ifdef SCARLET_EXECUTION_PROBE
#import "probe.h"
#else
extern void scarlet_ios_boot(const char *bundle, const char *root, void (*log)(const char *));
extern void scarlet_ios_run(const char *image, const char *elf, void (*log)(const char *));
#endif

static UITextView *output;

static void show_line(const char *line) {
    fprintf(stderr, "%s\n", line);
    NSString *message = [NSString stringWithUTF8String:line];
    dispatch_async(dispatch_get_main_queue(), ^{
        output.text = [output.text stringByAppendingFormat:@"%@\n", message];
    });
}

#ifndef SCARLET_EXECUTION_PROBE
static NSURL *prepare_root(NSBundle *bundle, NSError **error) {
    NSFileManager *manager = NSFileManager.defaultManager;
    NSURL *directory = [manager URLForDirectory:NSApplicationSupportDirectory
        inDomain:NSUserDomainMask appropriateForURL:nil create:YES error:error];
    if (!directory) {
        return nil;
    }
    NSURL *root = [directory URLByAppendingPathComponent:@"scarlet-root.ext2" isDirectory:NO];
    if ([manager fileExistsAtPath:root.path]) {
        return root;
    }
    NSURL *seed = [bundle.bundleURL URLByAppendingPathComponent:@"root.ext2" isDirectory:NO];
    NSURL *stage = [directory URLByAppendingPathComponent:NSUUID.UUID.UUIDString isDirectory:NO];
    if (![manager copyItemAtURL:seed toURL:stage error:error] ||
        ![manager moveItemAtURL:stage toURL:root error:error]) {
        [manager removeItemAtURL:stage error:nil];
        return nil;
    }
    return root;
}
#endif

@interface ProbeScene : UIResponder <UIWindowSceneDelegate>
@property(nonatomic, strong) UIWindow *window;
@end

@implementation ProbeScene
- (void)scene:(UIScene *)scene willConnectToSession:(UISceneSession *)session
      options:(UISceneConnectionOptions *)connectionOptions {
    (void)session;
    (void)connectionOptions;
    self.window = [[UIWindow alloc] initWithWindowScene:(UIWindowScene *)scene];
    UIViewController *controller = [UIViewController new];
    output = [UITextView new];
    output.editable = NO;
    output.font = [UIFont monospacedSystemFontOfSize:14 weight:UIFontWeightRegular];
    output.textContainerInset = UIEdgeInsetsMake(20, 16, 20, 16);
    output.text = @"";
    controller.view = output;
    self.window.rootViewController = controller;
    [self.window makeKeyAndVisible];

    static dispatch_once_t once;
    dispatch_once(&once, ^{
#ifdef SCARLET_EXECUTION_PROBE
        NSString *path = [NSBundle.mainBundle.privateFrameworksPath
            stringByAppendingPathComponent:@"ScarletFixture.framework/ScarletFixture"];
        dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
            bool passed = probe_run(path.fileSystemRepresentation, show_line);
            show_line(passed ? "SCARLET_P0_PASS" : "SCARLET_P0_FAIL");
        });
#else
        dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
            NSBundle *bundle = NSBundle.mainBundle;
            if ([NSProcessInfo.processInfo.arguments containsObject:@"--scheduler-check"]) {
                NSString *path = [bundle.privateFrameworksPath
                    stringByAppendingPathComponent:@"ScarletGuest.framework/ScarletGuest"];
                NSString *elf = [bundle.resourcePath stringByAppendingPathComponent:@"scheduler.elf"];
                scarlet_ios_run(path.fileSystemRepresentation, elf.fileSystemRepresentation, show_line);
            } else {
                NSError *error = nil;
                NSURL *root = prepare_root(bundle, &error);
                if (!root) {
                    NSString *message = [NSString stringWithFormat:@"SCARLET_BOOT_FAIL: %@", error];
                    show_line(message.UTF8String);
                    return;
                }
                scarlet_ios_boot(bundle.bundlePath.fileSystemRepresentation, root.fileSystemRepresentation, show_line);
            }
        });
#endif
    });
}
@end

@interface ProbeApp : UIResponder <UIApplicationDelegate>
@end
@implementation ProbeApp
@end

int main(int argc, char **argv) {
    @autoreleasepool {
        return UIApplicationMain(argc, argv, nil, NSStringFromClass(ProbeApp.class));
    }
}
