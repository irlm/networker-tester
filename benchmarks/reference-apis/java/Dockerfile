FROM eclipse-temurin:21-jdk AS build
WORKDIR /build
COPY Server.java .
# Pack ALL compiled classes: javac emits one .class per inner handler class
# (APIUsersHandler, AggregateHandler, ...); listing only three of them made
# the jar boot-fail with NoClassDefFoundError (audit V7).
RUN javac Server.java && \
    jar cfe server.jar Server *.class

FROM eclipse-temurin:21-jre
# NOT /opt/bench: the cert/dataset volume is mounted there at runtime and
# would shadow server.jar, making the container unbootable.
WORKDIR /app
COPY --from=build /build/server.jar .
# Certificates must be mounted at /opt/bench/cert.pem and /opt/bench/key.pem
EXPOSE 8443
CMD ["java", "-jar", "server.jar"]
