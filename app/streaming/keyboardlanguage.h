#pragma once

#include <QString>
#include <functional>

// The language of the keyboard input source the user has selected (macOS),
// so the host can type in it too. Elsewhere: empty, never changes.
namespace KeyboardLanguage
{
// "en", "ru", "zh-Hans", ... (empty when unknown)
QString current();

// Calls `changed` on the main thread whenever the user picks another input
// source; stopWatching() ends it.
void watch(std::function<void()> changed);
void stopWatching();
}
