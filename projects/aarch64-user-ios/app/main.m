#import <UIKit/UIKit.h>

#ifdef SCARLET_EXECUTION_PROBE
#import "probe.h"
#else
extern void scarlet_ios_boot(const char *bundle, const char *root, void (*log)(const char *));
extern bool scarlet_ios_console_send(const void *bytes, size_t length);
extern void scarlet_ios_run(const char *image, const char *elf, void (*log)(const char *));
#endif

static UITextView *output;

static void show_line(const char *line) {
    fprintf(stderr, "%s\n", line);
    NSString *message = [NSString stringWithUTF8String:line];
    dispatch_async(dispatch_get_main_queue(), ^{
        output.text = [output.text stringByAppendingFormat:@"%@\n", message];
        [output scrollRangeToVisible:NSMakeRange(output.text.length, 0)];
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

@interface ProbeScene : UIResponder <UIWindowSceneDelegate, UITextFieldDelegate>
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
#ifndef SCARLET_EXECUTION_PROBE
    bool schedulerCheck = [NSProcessInfo.processInfo.arguments containsObject:@"--scheduler-check"];
    if (!schedulerCheck) {
        controller.view = [UIView new];
        controller.view.backgroundColor = UIColor.systemBackgroundColor;
        UITextField *input = [UITextField new];
        input.borderStyle = UITextBorderStyleRoundedRect;
        input.placeholder = @"TTY input";
        input.accessibilityIdentifier = @"scarlet-console-input";
        input.autocorrectionType = UITextAutocorrectionTypeNo;
        input.autocapitalizationType = UITextAutocapitalizationTypeNone;
        input.spellCheckingType = UITextSpellCheckingTypeNo;
        input.returnKeyType = UIReturnKeySend;
        input.delegate = self;

        UIStackView *console = [[UIStackView alloc] initWithArrangedSubviews:@[output, input]];
        console.axis = UILayoutConstraintAxisVertical;
        console.spacing = 8;
        console.translatesAutoresizingMaskIntoConstraints = NO;
        [controller.view addSubview:console];
        UILayoutGuide *safe = controller.view.safeAreaLayoutGuide;
        [NSLayoutConstraint activateConstraints:@[
            [console.leadingAnchor constraintEqualToAnchor:safe.leadingAnchor constant:12],
            [console.trailingAnchor constraintEqualToAnchor:safe.trailingAnchor constant:-12],
            [console.topAnchor constraintEqualToAnchor:safe.topAnchor],
            [console.bottomAnchor constraintEqualToAnchor:controller.view.keyboardLayoutGuide.topAnchor constant:-8],
        ]];
    }
#endif
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
            if (schedulerCheck) {
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

#ifndef SCARLET_EXECUTION_PROBE
- (BOOL)textFieldShouldReturn:(UITextField *)input {
    NSString *line = [(input.text ?: @"") stringByAppendingString:@"\n"];
    NSData *bytes = [line dataUsingEncoding:NSUTF8StringEncoding];
    if (scarlet_ios_console_send(bytes.bytes, bytes.length)) {
        input.text = @"";
    }
    return NO;
}
#endif
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
