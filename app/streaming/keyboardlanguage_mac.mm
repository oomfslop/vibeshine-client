#include "keyboardlanguage.h"

#import <Carbon/Carbon.h>
#import <Foundation/Foundation.h>

namespace KeyboardLanguage
{

static id s_Observer = nil;

QString current()
{
    TISInputSourceRef source = TISCopyCurrentKeyboardInputSource();
    if (source == nullptr) {
        return QString();
    }
    QString language;
    CFArrayRef languages = (CFArrayRef)TISGetInputSourceProperty(source, kTISPropertyInputSourceLanguages);
    if (languages != nullptr && CFArrayGetCount(languages) > 0) {
        language = QString::fromCFString((CFStringRef)CFArrayGetValueAtIndex(languages, 0));
    }
    CFRelease(source);
    return language;
}

void watch(std::function<void()> changed)
{
    stopWatching();
    // (no ARC here: the observer token is kept retained)
    s_Observer = [[[NSDistributedNotificationCenter defaultCenter]
        addObserverForName:(__bridge NSString*)kTISNotifySelectedKeyboardInputSourceChanged
                    object:nil
                     queue:[NSOperationQueue mainQueue]
                usingBlock:^(NSNotification*) {
                    changed();
                }] retain];
}

void stopWatching()
{
    if (s_Observer != nil) {
        [[NSDistributedNotificationCenter defaultCenter] removeObserver:s_Observer];
        [s_Observer release];
        s_Observer = nil;
    }
}

}
