#import "probe.h"

#import <UIKit/UIKit.h>

static UITextView *output;

static void show_line(const char *line) {
    fprintf(stderr, "%s\n", line);
    NSString *message = [NSString stringWithUTF8String:line];
    dispatch_async(dispatch_get_main_queue(), ^{
        output.text = [output.text stringByAppendingFormat:@"%@\n", message];
    });
}

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
        NSString *path = [NSBundle.mainBundle.privateFrameworksPath
            stringByAppendingPathComponent:@"ScarletFixture.framework/ScarletFixture"];
        dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
            bool passed = probe_run(path.fileSystemRepresentation, show_line);
            show_line(passed ? "SCARLET_P0_PASS" : "SCARLET_P0_FAIL");
        });
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
