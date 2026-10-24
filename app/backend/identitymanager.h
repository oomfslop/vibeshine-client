#pragma once

#include <QSslConfiguration>
#include <QSslCertificate>
#include <QSslKey>
#include <QSettings>

class IdentityManager
{
public:
    QString
    getUniqueId();

    QByteArray
    getCertificate();

    QByteArray
    getPrivateKey();

    QSslConfiguration
    getSslConfig();

    static
    IdentityManager*
    get();

    // Uses the given credentials instead of the ones in QSettings. Must be
    // called before the first get(). Returns false if they are unreadable.
    static
    bool
    setOverride(const QByteArray& certPem, const QByteArray& keyPem, const QString& uniqueId);

private:
    IdentityManager();
    IdentityManager(const QByteArray& certPem, const QByteArray& keyPem, const QString& uniqueId);

    QSslCertificate
    getSslCertificate();

    QSslKey
    getSslKey();

    void
    createCredentials(QSettings& settings);

    // Initialized in constructor
    QByteArray m_CachedPrivateKey;
    QByteArray m_CachedPemCert;

    // Lazy initialized
    QString m_CachedUniqueId;
    QSslCertificate m_CachedSslCert;
    QSslKey m_CachedSslKey;

    static IdentityManager* s_Im;
};
