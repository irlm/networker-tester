FROM eclipse-temurin:21-jdk AS build
WORKDIR /build
COPY Server.java .
RUN javac Server.java && \
    jar cfe server.jar Server Server.class \
        'Server$HealthHandler.class' \
        'Server$DownloadHandler.class' \
        'Server$UploadHandler.class'

FROM eclipse-temurin:21-jre
WORKDIR /opt/bench
COPY --from=build /build/server.jar .
# Certificates must be mounted at /opt/bench/cert.pem and /opt/bench/key.pem
EXPOSE 8443
CMD ["java", "-jar", "server.jar"]
