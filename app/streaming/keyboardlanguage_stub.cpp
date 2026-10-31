#include "keyboardlanguage.h"

#ifndef Q_OS_DARWIN
namespace KeyboardLanguage
{
QString current()
{
    return QString();
}

void watch(std::function<void()>)
{
}

void stopWatching()
{
}
}
#endif
